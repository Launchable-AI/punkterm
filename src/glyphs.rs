//! Font loading, shaping and the glyph atlas. Terminal cells are shaped one
//! grapheme at a time and placed on the grid, so fallback fonts can never push
//! a line out of alignment; UI labels are shaped as whole runs.

use std::collections::HashMap;
use std::rc::Rc;

use cosmic_text::{
    Attrs, Buffer, CacheKey, Family, FontSystem, Metrics, Shaping, Style, SwashCache, SwashContent, Weight,
};

use crate::gfx::{Upload, ATLAS_SIZE};

#[derive(Clone, Copy, Debug)]
pub struct AtlasGlyph {
    pub ax: u32,
    pub ay: u32,
    pub w: u32,
    pub h: u32,
    pub left: i32,
    pub top: i32,
    pub color: bool,
}

/// A shaped glyph relative to a pen origin at the top-left of its line box.
#[derive(Clone, Copy, Debug)]
pub struct Placed {
    pub key: CacheKey,
    pub x: f32,
    pub y: f32,
}

pub struct TextLayout {
    pub glyphs: Vec<Placed>,
    pub width: f32,
    pub height: f32,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum Font {
    Mono,
    Ui,
}

pub struct Glyphs {
    fs: FontSystem,
    swash: SwashCache,
    mono: String,
    ui: String,
    pub mono_px: f32,
    pub cell_w: f32,
    pub cell_h: f32,
    atlas: HashMap<CacheKey, Option<AtlasGlyph>>,
    cells: HashMap<(String, u8), Option<Placed>>,
    runs: HashMap<(String, Font, u32, u16, i32), Rc<TextLayout>>,
    shelf: (u32, u32, u32),
    pub uploads: Vec<Upload>,
}

impl Glyphs {
    pub fn new(mono: &str, ui: &str, mono_px: f32) -> Self {
        let fs = FontSystem::new();
        // Not every machine has the Nerd Font: fall back to a monospace
        // family that is installed, so the grid never lands on a sans face.
        let has = |name: &str| fs.db().faces().any(|f| f.families.iter().any(|(n, _)| n == name));
        let mono = [mono, "JetBrainsMono Nerd Font Mono", "JetBrains Mono", "Ubuntu Mono", "DejaVu Sans Mono", "Liberation Mono", "Noto Sans Mono"]
            .into_iter()
            .find(|n| has(n))
            .map(str::to_string)
            .or_else(|| fs.db().faces().find(|f| f.monospaced).and_then(|f| f.families.first().map(|(n, _)| n.clone())))
            .unwrap_or_else(|| mono.to_string());
        let ui = if has(ui) { ui.to_string() } else { mono.clone() };
        let mut g = Glyphs {
            fs,
            swash: SwashCache::new(),
            mono,
            ui,
            mono_px,
            cell_w: 0.0,
            cell_h: 0.0,
            atlas: HashMap::new(),
            cells: HashMap::new(),
            runs: HashMap::new(),
            shelf: (1, 1, 0),
            uploads: Vec::new(),
        };
        g.set_size(mono_px);
        g
    }

    pub fn set_size(&mut self, mono_px: f32) {
        self.mono_px = mono_px;
        self.cells.clear();
        let line_h = (mono_px * 1.32).round();
        let mut buf = Buffer::new(&mut self.fs, Metrics::new(mono_px, line_h));
        let attrs = Attrs::new().family(Family::Name(&self.mono));
        buf.set_text("M", &attrs, Shaping::Advanced, None);
        buf.shape_until_scroll(&mut self.fs, false);
        let adv = buf.layout_runs().next().and_then(|r| r.glyphs.first().map(|g| g.w)).unwrap_or(mono_px * 0.6);
        self.cell_w = adv.round().max(1.0);
        self.cell_h = line_h;
    }

    fn family(&self, f: Font) -> &str {
        match f {
            Font::Mono => &self.mono,
            Font::Ui => &self.ui,
        }
    }

    /// The glyph for one terminal cell, positioned relative to the cell's top-left.
    pub fn cell(&mut self, text: &str, bold: bool, italic: bool) -> Option<Placed> {
        let style = bold as u8 | (italic as u8) << 1;
        if let Some(p) = self.cells.get(&(text.to_string(), style)) {
            return *p;
        }
        let mut buf = Buffer::new(&mut self.fs, Metrics::new(self.mono_px, self.cell_h));
        let mut attrs = Attrs::new().family(Family::Name(&self.mono));
        if bold {
            attrs = attrs.weight(Weight::BOLD);
        }
        if italic {
            attrs = attrs.style(Style::Italic);
        }
        buf.set_text(text, &attrs, Shaping::Advanced, None);
        buf.shape_until_scroll(&mut self.fs, false);
        let placed = buf.layout_runs().next().and_then(|run| {
            run.glyphs.first().map(|g| {
                let p = g.physical((0.0, 0.0), 1.0);
                Placed { key: p.cache_key, x: p.x as f32, y: run.line_y + p.y as f32 }
            })
        });
        self.cells.insert((text.to_string(), style), placed);
        placed
    }

    /// Shape a UI label. Cached by content, so call it freely every frame.
    pub fn text(&mut self, text: &str, font: Font, px: f32, weight: u16, tracking: f32) -> Rc<TextLayout> {
        let key = (text.to_string(), font, px.to_bits(), weight, (tracking * 100.0) as i32);
        if let Some(l) = self.runs.get(&key) {
            return l.clone();
        }
        let line_h = (px * 1.3).round();
        let mut buf = Buffer::new(&mut self.fs, Metrics::new(px, line_h));
        let fam = self.family(font).to_string();
        let attrs = Attrs::new().family(Family::Name(&fam)).weight(Weight(weight));
        buf.set_text(text, &attrs, Shaping::Advanced, None);
        buf.shape_until_scroll(&mut self.fs, false);
        let mut glyphs = Vec::new();
        let mut width: f32 = 0.0;
        if let Some(run) = buf.layout_runs().next() {
            for (i, g) in run.glyphs.iter().enumerate() {
                let p = g.physical((i as f32 * tracking, 0.0), 1.0);
                glyphs.push(Placed { key: p.cache_key, x: p.x as f32, y: run.line_y + p.y as f32 });
                width = width.max(g.x + g.w + i as f32 * tracking);
            }
        }
        let l = Rc::new(TextLayout { glyphs, width, height: line_h });
        self.runs.insert(key, l.clone());
        l
    }

    pub fn run_cache_len(&self) -> usize {
        self.runs.len()
    }

    pub fn clear_runs(&mut self) {
        self.runs.clear();
    }

    /// Rasterize a glyph into the atlas on first use.
    pub fn atlas(&mut self, key: CacheKey) -> Option<AtlasGlyph> {
        if let Some(a) = self.atlas.get(&key) {
            return *a;
        }
        let entry = self.rasterize(key);
        self.atlas.insert(key, entry);
        entry
    }

    fn rasterize(&mut self, key: CacheKey) -> Option<AtlasGlyph> {
        let img = self.swash.get_image_uncached(&mut self.fs, key)?;
        let (w, h) = (img.placement.width, img.placement.height);
        if w == 0 || h == 0 {
            return None;
        }
        let (rgba, color) = match img.content {
            SwashContent::Mask => (img.data.iter().flat_map(|&a| [255, 255, 255, a]).collect(), false),
            SwashContent::Color => (img.data.clone(), true),
            SwashContent::SubpixelMask => (
                img.data.chunks(4).flat_map(|c| [255, 255, 255, c[0].max(c[1]).max(c[2])]).collect(),
                false,
            ),
        };
        let (ax, ay) = self.alloc(w, h)?;
        self.uploads.push(Upload { x: ax, y: ay, w, h, rgba });
        Some(AtlasGlyph { ax, ay, w, h, left: img.placement.left, top: img.placement.top, color })
    }

    /// Shelf packing. When the atlas fills, start over: every glyph re-rasterizes
    /// lazily on the next frame.
    fn alloc(&mut self, w: u32, h: u32) -> Option<(u32, u32)> {
        let (mut x, mut y, mut row) = self.shelf;
        if x + w + 1 > ATLAS_SIZE {
            x = 1;
            y += row + 1;
            row = 0;
        }
        if y + h + 1 > ATLAS_SIZE {
            self.atlas.clear();
            self.shelf = (1, 1, 0);
            return self.alloc(w, h);
        }
        self.shelf = (x + w + 1, y, row.max(h));
        Some((x, y))
    }
}
