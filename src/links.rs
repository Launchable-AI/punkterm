//! Links in the terminal: OSC 8 hyperlinks, else URLs or file paths in the
//! text (joined across soft-wrapped rows). Ctrl+click opens them; a path on
//! an ssh host is downloaded first (`App::open_link`).

use std::process::{Command, Stdio};

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::Term;

pub struct Link {
    pub uri: String,
    /// A file path (absolute or `~/`), not a URL: it lives where the pane's
    /// program runs.
    pub is_path: bool,
    /// Every cell the link covers, for the hover underline.
    pub cells: Vec<Point>,
}

/// The link at `point`, if any.
pub fn at<T>(term: &Term<T>, point: Point) -> Option<Link> {
    let grid = term.grid();
    let cols = term.columns();
    let top = -(grid.history_size() as i32);
    let bottom = term.screen_lines() as i32 - 1;

    // An explicit hyperlink: the run of cells that share it, followed onto
    // the rows above and below where it runs on from a row's end to the
    // next one's start (a long URL Claude Code broke at the pane's width).
    if let Some(h) = grid[point].hyperlink() {
        let same = |l: i32, c: usize| grid[Line(l)][Column(c)].hyperlink().is_some_and(|o| o == h);
        let (mut a, mut b) = (point.column.0, point.column.0);
        while a > 0 && same(point.line.0, a - 1) {
            a -= 1;
        }
        while b + 1 < cols && same(point.line.0, b + 1) {
            b += 1;
        }
        let mut cells: Vec<Point> = (a..=b).map(|c| Point::new(point.line, Column(c))).collect();
        // Up: while this run starts its row and the row above ends in it.
        let (mut l, mut start) = (point.line.0, a);
        while start == 0 && l > top && same(l - 1, cols - 1) {
            l -= 1;
            start = (0..cols).rev().take_while(|&c| same(l, c)).last().unwrap_or(cols - 1);
            cells.splice(0..0, (start..cols).map(|c| Point::new(Line(l), Column(c))));
        }
        // Down: while this run ends its row and the row below starts in it.
        let (mut l, mut end) = (point.line.0, b);
        while end == cols - 1 && l < bottom && same(l + 1, 0) {
            l += 1;
            end = (0..cols).take_while(|&c| same(l, c)).last().unwrap_or(0);
            cells.extend((0..=end).map(|c| Point::new(Line(l), Column(c))));
        }
        return allowed(h.uri()).then(|| Link { uri: h.uri().to_string(), is_path: false, cells });
    }

    // Plain text: the logical line through `point`, rows joined where they
    // wrap: where the terminal wrapped them, or where a program broke a long
    // line itself (Claude Code does, at the pane's width): a row that runs
    // to its last column into one that starts at its first.
    let ink = |l: i32, c: usize| !grid[Line(l)][Column(c)].c.is_whitespace();
    let wraps = |l: i32| {
        let last = &grid[Line(l)][Column(cols - 1)];
        last.flags.contains(Flags::WRAPLINE) || (l < bottom && ink(l, cols - 1) && ink(l + 1, 0))
    };
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
    let click = click?;
    if let Some((a, b)) = find_url(&text, click) {
        let uri: String = text[a..b].iter().collect();
        return Some(Link { uri, is_path: false, cells: at[a..b].to_vec() });
    }
    let (a, b) = find_path(&text, click)?;
    let uri: String = text[a..b].iter().collect();
    Some(Link { uri, is_path: true, cells: at[a..b].to_vec() })
}

/// The file path (`/…` or `~/…`) in `text` covering index `i`, as a char
/// range: the token around it, less a trailing `:line:col` and punctuation.
fn find_path(text: &[char], i: usize) -> Option<(usize, usize)> {
    let stop = |c: char| c.is_whitespace() || "<>\"'`|(){}[]".contains(c) || (c as u32) < 0x20;
    if stop(*text.get(i)?) {
        return None;
    }
    let mut a = i;
    while a > 0 && !stop(text[a - 1]) {
        a -= 1;
    }
    let mut b = i;
    while b < text.len() && !stop(text[b]) {
        b += 1;
    }
    // Where the path starts inside the token ("file=/x", "→/x"): its slash.
    let start = (a..b).find(|&k| text[k] == '/' || (text[k] == '~' && text.get(k + 1) == Some(&'/')))?;
    if start > a && !"=:→".contains(text[start - 1]) {
        return None;
    }
    let a = start;
    // file.rs:12:5 opens file.rs; a sentence's full stop is not the path's.
    loop {
        let tail: String = text[a..b].iter().collect();
        if let Some(k) = tail.rfind(':').filter(|&k| k + 1 < tail.len() && tail[k + 1..].chars().all(|c| c.is_ascii_digit())) {
            b = a + tail[..k].chars().count();
        } else if text[a..b].last().is_some_and(|c| ".,;:!?".contains(*c)) {
            b -= 1;
        } else {
            break;
        }
    }
    let path: String = text[a..b].iter().collect();
    let body = path.trim_start_matches('~');
    (i >= a && i < b && body.len() > 1 && body[1..].chars().any(|c| c.is_alphanumeric())).then_some((a, b))
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

    fn path(s: &str, at: &str) -> Option<String> {
        let chars: Vec<char> = s.chars().collect();
        let i = s[..s.find(at).unwrap()].chars().count();
        find_path(&chars, i).map(|(a, b)| chars[a..b].iter().collect())
    }

    #[test]
    fn finds_paths() {
        assert_eq!(path("The file is at /tmp/x/shot.png.", "shot").as_deref(), Some("/tmp/x/shot.png"));
        assert_eq!(path("see src=/home/me/a.rs:12:5 now", "a.rs").as_deref(), Some("/home/me/a.rs"));
        assert_eq!(path("open ~/Downloads/report.pdf", "report").as_deref(), Some("~/Downloads/report.pdf"));
        assert_eq!(path("(/var/log/syslog)", "syslog").as_deref(), Some("/var/log/syslog"));
        assert_eq!(path("and/or this", "or"), None);
        assert_eq!(path("a / b", "/"), None);
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
