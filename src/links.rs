//! Links in the terminal: OSC 8 hyperlinks, else URLs in the text (joined
//! across soft-wrapped rows). Ctrl+click opens them.

use std::process::{Command, Stdio};

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::Term;

pub struct Link {
    pub uri: String,
    /// Every cell the link covers, for the hover underline.
    pub cells: Vec<Point>,
}

/// The link at `point`, if any.
pub fn at<T>(term: &Term<T>, point: Point) -> Option<Link> {
    let grid = term.grid();
    let cols = term.columns();
    let top = -(grid.history_size() as i32);
    let bottom = term.screen_lines() as i32 - 1;

    // An explicit hyperlink: the run of cells on this row that share it.
    if let Some(h) = grid[point].hyperlink() {
        let row = &grid[point.line];
        let same = |c: usize| row[Column(c)].hyperlink().is_some_and(|o| o == h);
        let (mut a, mut b) = (point.column.0, point.column.0);
        while a > 0 && same(a - 1) {
            a -= 1;
        }
        while b + 1 < cols && same(b + 1) {
            b += 1;
        }
        let cells = (a..=b).map(|c| Point::new(point.line, Column(c))).collect();
        return allowed(h.uri()).then(|| Link { uri: h.uri().to_string(), cells });
    }

    // Plain text: the logical line through `point`, rows joined where they wrap.
    let wraps = |l: i32| grid[Line(l)][Column(cols - 1)].flags.contains(Flags::WRAPLINE);
    let mut first = point.line.0;
    while first > top && wraps(first - 1) {
        first -= 1;
    }
    let mut last = point.line.0;
    while last < bottom && wraps(last) {
        last += 1;
    }
    let mut text = Vec::new();
    let mut at = Vec::new();
    let mut click = None;
    for l in first..=last {
        for c in 0..cols {
            let cell = &grid[Line(l)][Column(c)];
            if cell.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
                continue;
            }
            if l == point.line.0 && c == point.column.0 {
                click = Some(text.len());
            }
            text.push(cell.c);
            at.push(Point::new(Line(l), Column(c)));
        }
    }
    let (a, b) = find_url(&text, click?)?;
    let uri: String = text[a..b].iter().collect();
    Some(Link { uri, cells: at[a..b].to_vec() })
}

/// Schemes worth handing to the desktop. An OSC 8 link can name anything.
fn allowed(uri: &str) -> bool {
    ["http://", "https://", "mailto:", "file://"].iter().any(|s| uri.starts_with(s))
}

/// The http(s) URL in `text` covering index `i`, as a char range.
fn find_url(text: &[char], i: usize) -> Option<(usize, usize)> {
    let stop = |c: char| c.is_whitespace() || "<>\"`{}|\\^".contains(c) || (c as u32) < 0x20;
    // Back up to the start of the token, then look for a scheme in it.
    let mut start = i;
    while start > 0 && !stop(text[start - 1]) {
        start -= 1;
    }
    let token: String = text[start..].iter().take_while(|c| !stop(**c)).collect();
    let off = ["https://", "http://"].iter().filter_map(|s| token.find(s)).min()?;
    let a = start + token[..off].chars().count();
    let mut b = start + token.chars().count();
    // Trailing punctuation belongs to the sentence, and a closing bracket
    // only to the URL when it opened one.
    loop {
        let Some(&c) = text[a..b].last() else { return None };
        let unbalanced = |open: char, close: char| {
            c == close && text[a..b].iter().filter(|&&x| x == open).count() < text[a..b].iter().filter(|&&x| x == close).count()
        };
        if ".,;:!?'".contains(c) || unbalanced('(', ')') || unbalanced('[', ']') {
            b -= 1;
        } else {
            break;
        }
    }
    (i >= a && i < b && b - a > "https://".len()).then_some((a, b))
}

/// Hand `uri` to the desktop's opener, detached.
pub fn open(uri: &str) {
    let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    let _ = Command::new(opener).arg(uri).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str, at: &str) -> Option<String> {
        let chars: Vec<char> = s.chars().collect();
        let i = s[..s.find(at).unwrap()].chars().count();
        find_url(&chars, i).map(|(a, b)| chars[a..b].iter().collect())
    }

    #[test]
    fn finds_urls() {
        assert_eq!(url("see https://example.com/a?b=1 now", "example").as_deref(), Some("https://example.com/a?b=1"));
        assert_eq!(url("(https://example.com/x).", "example").as_deref(), Some("https://example.com/x"));
        assert_eq!(url("https://en.wikipedia.org/wiki/Rust_(language), ok", "wiki").as_deref(), Some("https://en.wikipedia.org/wiki/Rust_(language)"));
        assert_eq!(url("→https://example.com", "https").as_deref(), Some("https://example.com"));
        assert_eq!(url("no link here", "link"), None);
        assert_eq!(url("see https://example.com now", "now"), None);
    }
}
