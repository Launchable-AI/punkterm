//! Inline images: the kitty graphics protocol, as Claude Code uses it.
//!
//! Images arrive as APC commands (`ESC _ G <keys> ; <base64> ESC \`), from
//! inside tmux wrapped in passthrough (`ESC P tmux; ... ESC \`, its ESCs
//! doubled). tmux's control mode hands punkterm a pane's raw output, so both
//! reach here whatever tmux itself does with passthrough. Claude places
//! images with Unicode placeholders: cells holding U+10EEEE whose colour
//! names the image and whose combining marks give the row and column of the
//! image they show; punkterm draws that slice of the image in each.

use std::collections::HashMap;
use std::sync::Arc;

pub const PLACEHOLDER: char = '\u{10EEEE}';

/// Largest side an image is kept at: bigger ones are scaled down on arrival.
const MAX_SIDE: u32 = 1536;

pub struct Image {
    pub w: u32,
    pub h: u32,
    pub rgba: Arc<Vec<u8>>,
    /// The placement's size in cells (0: from the image's own size).
    pub cols: u32,
    pub rows: u32,
    /// Changes whenever the image is replaced, so caches refresh.
    pub version: u64,
}

/// What a byte is part of, between calls (sequences span reads).
#[derive(Default)]
enum Scan {
    #[default]
    Ground,
    Esc,
    Apc,
    ApcEsc,
    /// A DCS: its first bytes, to tell tmux passthrough from anything else.
    DcsHead,
    Pass,
    PassEsc,
}

#[derive(Default)]
struct Scanner {
    state: Scan,
    buf: Vec<u8>,
}

/// One scan's result: bytes for the terminal, and kitty commands (the APC
/// body after `G`).
#[derive(Default)]
struct Scanned {
    out: Vec<u8>,
    cmds: Vec<Vec<u8>>,
}

const ESC: u8 = 0x1b;
/// A sequence longer than this is not one we want: let it go.
const MAX_SEQ: usize = 64 << 20;

impl Scanner {
    fn scan(&mut self, input: &[u8], r: &mut Scanned) {
        for &b in input {
            match self.state {
                Scan::Ground => {
                    if b == ESC {
                        self.state = Scan::Esc;
                    } else {
                        r.out.push(b);
                    }
                }
                Scan::Esc => match b {
                    b'_' => {
                        self.state = Scan::Apc;
                        self.buf.clear();
                    }
                    b'P' => {
                        self.state = Scan::DcsHead;
                        self.buf.clear();
                    }
                    ESC => r.out.push(ESC),
                    _ => {
                        r.out.extend_from_slice(&[ESC, b]);
                        self.state = Scan::Ground;
                    }
                },
                Scan::Apc => match b {
                    ESC => self.state = Scan::ApcEsc,
                    0x07 => self.end_apc(r),
                    _ => self.push(b, r),
                },
                Scan::ApcEsc => {
                    if b == b'\\' {
                        self.end_apc(r);
                    } else {
                        self.push(ESC, r);
                        self.push(b, r);
                        self.state = Scan::Apc;
                    }
                }
                Scan::DcsHead => {
                    self.buf.push(b);
                    if !b"tmux;".starts_with(&self.buf) {
                        // Some other DCS: the terminal's business.
                        r.out.extend_from_slice(&[ESC, b'P']);
                        r.out.extend_from_slice(&self.buf);
                        self.state = Scan::Ground;
                    } else if self.buf.len() == 5 {
                        self.buf.clear();
                        self.state = Scan::Pass;
                    }
                }
                Scan::Pass => {
                    if b == ESC {
                        self.state = Scan::PassEsc;
                    } else {
                        self.push(b, r);
                    }
                }
                Scan::PassEsc => {
                    if b == ESC {
                        // A doubled ESC is one ESC of the wrapped sequence.
                        self.push(ESC, r);
                        self.state = Scan::Pass;
                    } else {
                        // ESC \ ends the passthrough: scan what it carried.
                        self.state = Scan::Ground;
                        let inner = std::mem::take(&mut self.buf);
                        let mut sub = Scanner::default();
                        sub.scan(&inner, r);
                        if b != b'\\' {
                            r.out.push(b);
                        }
                    }
                }
            }
        }
    }

    fn push(&mut self, b: u8, r: &mut Scanned) {
        self.buf.push(b);
        if self.buf.len() > MAX_SEQ {
            self.buf.clear();
            self.state = Scan::Ground;
            let _ = r;
        }
    }

    fn end_apc(&mut self, r: &mut Scanned) {
        self.state = Scan::Ground;
        let body = std::mem::take(&mut self.buf);
        if body.first() == Some(&b'G') {
            r.cmds.push(body[1..].to_vec());
        }
    }
}

/// A transmission still arriving in chunks (`m=1`).
struct Pending {
    keys: HashMap<u8, String>,
    data: Vec<u8>,
}

/// An image sent as a file path (`t=f`, or `t=t` to delete after): it is
/// on the machine the program runs on, which may be an ssh host, so the
/// pane's owner reads it and hands it back to `file_arrived`.
pub struct FileRequest {
    pub path: String,
    pub delete: bool,
    keys: HashMap<u8, String>,
}

#[derive(Default)]
pub struct Graphics {
    scanner: Scanner,
    pending: Option<Pending>,
    pub images: HashMap<u32, Image>,
    version: u64,
    /// Files to read for images sent by path.
    pub files: Vec<FileRequest>,
}

impl Graphics {
    /// Take the graphics out of `input`: the bytes for the terminal, and
    /// replies to write back to the program.
    pub fn filter(&mut self, input: &[u8]) -> (Vec<u8>, Vec<u8>) {
        // Nothing to look at: the common case, straight through.
        if matches!(self.scanner.state, Scan::Ground) && !input.contains(&ESC) {
            return (input.to_vec(), Vec::new());
        }
        let mut r = Scanned { out: Vec::with_capacity(input.len()), cmds: Vec::new() };
        self.scanner.scan(input, &mut r);
        let mut replies = Vec::new();
        for c in r.cmds {
            if let Some(reply) = self.command(&c) {
                replies.extend_from_slice(reply.as_bytes());
            }
        }
        (r.out, replies)
    }

    fn command(&mut self, body: &[u8]) -> Option<String> {
        let (ctrl, payload) = match body.iter().position(|&b| b == b';') {
            Some(i) => (&body[..i], &body[i + 1..]),
            None => (body, &[][..]),
        };
        let mut keys: HashMap<u8, String> = HashMap::new();
        for kv in ctrl.split(|&b| b == b',') {
            if let [k, b'=', v @ ..] = kv {
                keys.insert(*k, String::from_utf8_lossy(v).into_owned());
            }
        }
        let more = keys.get(&b'm').is_some_and(|m| m == "1");
        // The next chunk of a transmission under way.
        if let Some(p) = self.pending.as_mut() {
            p.data.extend_from_slice(payload);
            if more {
                return None;
            }
            let p = self.pending.take()?;
            return self.transmit(p.keys, &p.data);
        }
        let num = |k: u8| keys.get(&k).and_then(|v| v.parse::<u32>().ok()).unwrap_or(0);
        match keys.get(&b'a').map(String::as_str).unwrap_or("t") {
            "q" => {
                let ok = keys.get(&b't').is_none_or(|t| matches!(t.as_str(), "d" | "f" | "t"));
                let msg = if ok { "OK" } else { "ENOTSUP:punkterm takes images inline only" };
                (num(b'q') < 2).then(|| format!("\x1b_Gi={};{msg}\x1b\\", num(b'i')))
            }
            "t" | "T" => {
                if more {
                    self.pending = Some(Pending { keys, data: payload.to_vec() });
                    return None;
                }
                self.transmit(keys, payload)
            }
            "p" => {
                if let Some(img) = self.images.get_mut(&num(b'i')) {
                    img.cols = num(b'c');
                    img.rows = num(b'r');
                }
                None
            }
            "d" => {
                match keys.get(&b'd').map(String::as_str).unwrap_or("a") {
                    "i" | "I" => {
                        self.images.remove(&num(b'i'));
                    }
                    "a" | "A" => self.images.clear(),
                    _ => {}
                }
                None
            }
            _ => None,
        }
    }

    fn transmit(&mut self, keys: HashMap<u8, String>, b64: &[u8]) -> Option<String> {
        let Some(data) = base64(b64) else { return reply(&keys, "EINVAL:bad base64") };
        match keys.get(&b't').map(String::as_str).unwrap_or("d") {
            "d" => self.store(keys, data),
            t @ ("f" | "t") => {
                let path = String::from_utf8_lossy(&data).into_owned();
                self.files.push(FileRequest { path, delete: t == "t", keys });
                None
            }
            _ => reply(&keys, "ENOTSUP:shared memory is not shared with punkterm"),
        }
    }

    /// A file asked for by `files` was read (`None`: it could not be).
    pub fn file_arrived(&mut self, req: FileRequest, data: Option<Vec<u8>>) -> Option<String> {
        match data {
            Some(d) => self.store(req.keys, d),
            None => reply(&req.keys, "ENOENT:could not read the file"),
        }
    }

    fn store(&mut self, keys: HashMap<u8, String>, mut data: Vec<u8>) -> Option<String> {
        let num = |k: u8| keys.get(&k).and_then(|v| v.parse::<u32>().ok()).unwrap_or(0);
        let id = num(b'i');
        if keys.get(&b'o').is_some_and(|o| o == "z") {
            use std::io::Read;
            let mut out = Vec::new();
            if flate2::read::ZlibDecoder::new(&data[..]).read_to_end(&mut out).is_err() {
                return reply(&keys, "EINVAL:bad zlib data");
            }
            data = out;
        }
        let decoded = match num(b'f') {
            0 | 32 => raw(&data, num(b's'), num(b'v'), 4),
            24 => raw(&data, num(b's'), num(b'v'), 3),
            100 => png_rgba(&data),
            _ => None,
        };
        let Some((w, h, rgba)) = decoded else { return reply(&keys, "EINVAL:could not decode the image") };
        let (w, h, rgba) = shrink(w, h, rgba, MAX_SIDE);
        self.version += 1;
        let place = keys.get(&b'a').is_some_and(|a| a == "T");
        let (cols, rows) = if place { (num(b'c'), num(b'r')) } else { self.images.get(&id).map_or((0, 0), |i| (i.cols, i.rows)) };
        self.images.insert(id, Image { w, h, rgba: Arc::new(rgba), cols, rows, version: self.version });
        reply(&keys, "OK")
    }
}

/// The reply to a transmission, as its `q` asks: none at 2, errors only at 1.
fn reply(keys: &HashMap<u8, String>, msg: &str) -> Option<String> {
    let num = |k: u8| keys.get(&k).and_then(|v| v.parse::<u32>().ok()).unwrap_or(0);
    let (id, quiet) = (num(b'i'), num(b'q'));
    let ok = msg == "OK";
    (id != 0 && (quiet == 0 || (quiet == 1 && !ok))).then(|| format!("\x1b_Gi={id};{msg}\x1b\\"))
}

/// Where images sit in the GPU's image atlas: packed in shelves as they are
/// first drawn. When it fills, it starts over on the next frame and the
/// images in sight are packed again.
#[derive(Default)]
pub struct Atlas {
    at: HashMap<(u64, u32), (u64, [f32; 4])>,
    x: u32,
    y: u32,
    row_h: u32,
    full: bool,
    pub uploads: Vec<crate::gfx::Upload>,
}

impl Atlas {
    pub fn begin_frame(&mut self) {
        if std::mem::take(&mut self.full) {
            self.at.clear();
            (self.x, self.y, self.row_h) = (0, 0, 0);
        }
    }

    /// Image `id` of pane `pane` in the atlas: x, y, w, h (px).
    pub fn place(&mut self, pane: u64, id: u32, img: &Image) -> Option<[f32; 4]> {
        if let Some(&(v, r)) = self.at.get(&(pane, id)) {
            if v == img.version {
                return Some(r);
            }
        }
        let size = crate::gfx::ATLAS_SIZE;
        let (w, h) = (img.w + 1, img.h + 1);
        if self.x + w > size {
            (self.x, self.y, self.row_h) = (0, self.y + self.row_h, 0);
        }
        if self.y + h > size || w > size {
            self.full = true;
            return None;
        }
        let r = [self.x as f32, self.y as f32, img.w as f32, img.h as f32];
        self.uploads.push(crate::gfx::Upload { x: self.x, y: self.y, w: img.w, h: img.h, rgba: img.rgba.to_vec() });
        self.x += w;
        self.row_h = self.row_h.max(h);
        self.at.insert((pane, id), (img.version, r));
        Some(r)
    }
}

/// Raw pixels, RGB or RGBA, as RGBA.
fn raw(data: &[u8], w: u32, h: u32, bpp: usize) -> Option<(u32, u32, Vec<u8>)> {
    let n = (w as usize).checked_mul(h as usize)?;
    if w == 0 || h == 0 || data.len() < n * bpp {
        return None;
    }
    if bpp == 4 {
        return Some((w, h, data[..n * 4].to_vec()));
    }
    let mut out = Vec::with_capacity(n * 4);
    for px in data[..n * 3].chunks_exact(3) {
        out.extend_from_slice(&[px[0], px[1], px[2], 255]);
    }
    Some((w, h, out))
}

fn png_rgba(data: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    let mut dec = png::Decoder::new(std::io::Cursor::new(data));
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = dec.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    let (w, h) = (info.width, info.height);
    let px = &buf[..info.buffer_size()];
    let rgba = match info.color_type {
        png::ColorType::Rgba => px.to_vec(),
        png::ColorType::Rgb => px.chunks_exact(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
        png::ColorType::GrayscaleAlpha => px.chunks_exact(2).flat_map(|p| [p[0], p[0], p[0], p[1]]).collect(),
        png::ColorType::Grayscale => px.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::Indexed => return None,
    };
    Some((w, h, rgba))
}

/// Scale down (box filter) so neither side exceeds `max`.
fn shrink(w: u32, h: u32, rgba: Vec<u8>, max: u32) -> (u32, u32, Vec<u8>) {
    if w <= max && h <= max {
        return (w, h, rgba);
    }
    let k = (w.max(h) as f32 / max as f32).ceil() as u32;
    let (nw, nh) = (w.div_ceil(k), h.div_ceil(k));
    let mut out = vec![0u8; (nw * nh * 4) as usize];
    for y in 0..nh {
        for x in 0..nw {
            let mut acc = [0u32; 4];
            let mut n = 0;
            for sy in y * k..((y + 1) * k).min(h) {
                for sx in x * k..((x + 1) * k).min(w) {
                    let i = ((sy * w + sx) * 4) as usize;
                    for c in 0..4 {
                        acc[c] += rgba[i + c] as u32;
                    }
                    n += 1;
                }
            }
            let o = ((y * nw + x) * 4) as usize;
            for c in 0..4 {
                out[o + c] = (acc[c] / n.max(1)) as u8;
            }
        }
    }
    (nw, nh, out)
}

fn base64(input: &[u8]) -> Option<Vec<u8>> {
    let val = |c: u8| -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            _ => return None,
        } as u32)
    };
    let mut out = Vec::with_capacity(input.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0);
    for &c in input {
        if c == b'=' || c.is_ascii_whitespace() {
            continue;
        }
        acc = (acc << 6) | val(c)?;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

/// Row/column marks, by value (kitty's rowcolumn-diacritics.txt).
const DIACRITICS: [u32; 297] = [
    0x0305, 0x030D, 0x030E, 0x0310, 0x0312, 0x033D, 0x033E, 0x033F, 0x0346, 0x034A, 0x034B, 0x034C, 
    0x0350, 0x0351, 0x0352, 0x0357, 0x035B, 0x0363, 0x0364, 0x0365, 0x0366, 0x0367, 0x0368, 0x0369, 
    0x036A, 0x036B, 0x036C, 0x036D, 0x036E, 0x036F, 0x0483, 0x0484, 0x0485, 0x0486, 0x0487, 0x0592, 
    0x0593, 0x0594, 0x0595, 0x0597, 0x0598, 0x0599, 0x059C, 0x059D, 0x059E, 0x059F, 0x05A0, 0x05A1, 
    0x05A8, 0x05A9, 0x05AB, 0x05AC, 0x05AF, 0x05C4, 0x0610, 0x0611, 0x0612, 0x0613, 0x0614, 0x0615, 
    0x0616, 0x0617, 0x0657, 0x0658, 0x0659, 0x065A, 0x065B, 0x065D, 0x065E, 0x06D6, 0x06D7, 0x06D8, 
    0x06D9, 0x06DA, 0x06DB, 0x06DC, 0x06DF, 0x06E0, 0x06E1, 0x06E2, 0x06E4, 0x06E7, 0x06E8, 0x06EB, 
    0x06EC, 0x0730, 0x0732, 0x0733, 0x0735, 0x0736, 0x073A, 0x073D, 0x073F, 0x0740, 0x0741, 0x0743, 
    0x0745, 0x0747, 0x0749, 0x074A, 0x07EB, 0x07EC, 0x07ED, 0x07EE, 0x07EF, 0x07F0, 0x07F1, 0x07F3, 
    0x0816, 0x0817, 0x0818, 0x0819, 0x081B, 0x081C, 0x081D, 0x081E, 0x081F, 0x0820, 0x0821, 0x0822, 
    0x0823, 0x0825, 0x0826, 0x0827, 0x0829, 0x082A, 0x082B, 0x082C, 0x082D, 0x0951, 0x0953, 0x0954, 
    0x0F82, 0x0F83, 0x0F86, 0x0F87, 0x135D, 0x135E, 0x135F, 0x17DD, 0x193A, 0x1A17, 0x1A75, 0x1A76, 
    0x1A77, 0x1A78, 0x1A79, 0x1A7A, 0x1A7B, 0x1A7C, 0x1B6B, 0x1B6D, 0x1B6E, 0x1B6F, 0x1B70, 0x1B71, 
    0x1B72, 0x1B73, 0x1CD0, 0x1CD1, 0x1CD2, 0x1CDA, 0x1CDB, 0x1CE0, 0x1DC0, 0x1DC1, 0x1DC3, 0x1DC4, 
    0x1DC5, 0x1DC6, 0x1DC7, 0x1DC8, 0x1DC9, 0x1DCB, 0x1DCC, 0x1DD1, 0x1DD2, 0x1DD3, 0x1DD4, 0x1DD5, 
    0x1DD6, 0x1DD7, 0x1DD8, 0x1DD9, 0x1DDA, 0x1DDB, 0x1DDC, 0x1DDD, 0x1DDE, 0x1DDF, 0x1DE0, 0x1DE1, 
    0x1DE2, 0x1DE3, 0x1DE4, 0x1DE5, 0x1DE6, 0x1DFE, 0x20D0, 0x20D1, 0x20D4, 0x20D5, 0x20D6, 0x20D7, 
    0x20DB, 0x20DC, 0x20E1, 0x20E7, 0x20E9, 0x20F0, 0x2CEF, 0x2CF0, 0x2CF1, 0x2DE0, 0x2DE1, 0x2DE2, 
    0x2DE3, 0x2DE4, 0x2DE5, 0x2DE6, 0x2DE7, 0x2DE8, 0x2DE9, 0x2DEA, 0x2DEB, 0x2DEC, 0x2DED, 0x2DEE, 
    0x2DEF, 0x2DF0, 0x2DF1, 0x2DF2, 0x2DF3, 0x2DF4, 0x2DF5, 0x2DF6, 0x2DF7, 0x2DF8, 0x2DF9, 0x2DFA, 
    0x2DFB, 0x2DFC, 0x2DFD, 0x2DFE, 0x2DFF, 0xA66F, 0xA67C, 0xA67D, 0xA6F0, 0xA6F1, 0xA8E0, 0xA8E1, 
    0xA8E2, 0xA8E3, 0xA8E4, 0xA8E5, 0xA8E6, 0xA8E7, 0xA8E8, 0xA8E9, 0xA8EA, 0xA8EB, 0xA8EC, 0xA8ED, 
    0xA8EE, 0xA8EF, 0xA8F0, 0xA8F1, 0xAAB0, 0xAAB2, 0xAAB3, 0xAAB7, 0xAAB8, 0xAABE, 0xAABF, 0xAAC1, 
    0xFE20, 0xFE21, 0xFE22, 0xFE23, 0xFE24, 0xFE25, 0xFE26, 0x10A0F, 0x10A38, 0x1D185, 0x1D186, 0x1D187, 
    0x1D188, 0x1D189, 0x1D1AA, 0x1D1AB, 0x1D1AC, 0x1D1AD, 0x1D242, 0x1D243, 0x1D244,
];

/// A combining mark's number in the placeholder scheme.
pub fn diacritic(c: char) -> Option<u32> {
    DIACRITICS.binary_search(&(c as u32)).ok().map(|i| i as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b64(data: &[u8]) -> String {
        const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut s = String::new();
        for ch in data.chunks(3) {
            let n = (ch[0] as u32) << 16 | (*ch.get(1).unwrap_or(&0) as u32) << 8 | *ch.get(2).unwrap_or(&0) as u32;
            for k in 0..=ch.len() {
                s.push(T[(n >> (18 - 6 * k) & 63) as usize] as char);
            }
        }
        s
    }

    #[test]
    fn query_and_chunked_transmit() {
        let mut g = Graphics::default();
        let (out, reply) = g.filter(b"a\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\b");
        assert_eq!(out, b"ab");
        assert_eq!(reply, b"\x1b_Gi=31;OK\x1b\\");
        // A 2x1 RGBA image in two chunks, split mid-sequence across reads.
        let px = b64(&[255, 0, 0, 255, 0, 255, 0, 255]);
        let (a, b) = px.split_at(4);
        let first = format!("\x1b_Ga=T,U=1,q=2,f=32,s=2,v=1,i=7,c=3,r=2,m=1;{a}\x1b\\\x1b_Gm=0;{b}\x1b\\");
        let (s1, s2) = first.as_bytes().split_at(10);
        assert!(g.filter(s1).0.is_empty());
        let (out, reply) = g.filter(s2);
        assert!(out.is_empty() && reply.is_empty());
        let img = &g.images[&7];
        assert_eq!((img.w, img.h, img.cols, img.rows), (2, 1, 3, 2));
        assert_eq!(&img.rgba[..4], &[255, 0, 0, 255]);
    }

    #[test]
    fn tmux_passthrough() {
        let mut g = Graphics::default();
        let inner = "\x1b_Gi=5,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\".replace('\x1b', "\x1b\x1b");
        let (out, reply) = g.filter(format!("x\x1bPtmux;{inner}\x1b\\y\x1b[1mz").as_bytes());
        assert_eq!(out, b"xy\x1b[1mz");
        assert_eq!(reply, b"\x1b_Gi=5;OK\x1b\\");
    }

    #[test]
    fn diacritics() {
        assert_eq!(diacritic('\u{0305}'), Some(0));
        assert_eq!(diacritic('\u{030D}'), Some(1));
        assert_eq!(diacritic('a'), None);
    }
}
