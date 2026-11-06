//! Sending files into a session: dropped on the window, or picked with the
//! UPLOAD button / Ctrl+Shift+F. On a remote host the file is copied there
//! over the host's shared ssh connection first; then its path is pasted,
//! as an `@` mention for Claude Code (which reads the file into context),
//! a bare path for an image (Claude attaches it), or quoted for a shell.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Where uploads land on a remote host: one private folder per upload, so
/// the file keeps its own name.
pub const REMOTE_DIR: &str = "/tmp/punkterm-upload";

const IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp"];

pub fn is_image(name: &str) -> bool {
    name.rsplit_once('.').is_some_and(|(_, e)| IMAGE_EXTS.contains(&e.to_ascii_lowercase().as_str()))
}

/// A file name safe to put unquoted in a shell command and an `@` mention.
pub fn safe_name(name: &str) -> String {
    let s: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || "._-+".contains(c) { c } else { '_' }).collect();
    let s = s.trim_start_matches(['.', '-']).to_string();
    if s.is_empty() { "file".to_string() } else { s }
}

/// A path needs no quoting in a shell or an `@` mention.
pub fn plain_path(p: &str) -> bool {
    !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric() || "/._-+~".contains(c))
}

/// What to paste for a file at `path` on the session's host.
pub fn mention(path: &str, claude: bool, image: bool) -> String {
    match (claude, image) {
        (true, true) => format!("{path} "),
        (true, false) => format!("@{path} "),
        (false, _) if plain_path(path) => format!("{path} "),
        (false, _) => format!("'{}' ", path.replace('\'', r"'\''")),
    }
}

/// Write `data` to `path` on the host behind ssh `spec`, in a fresh private
/// folder. Err says why not.
pub fn ssh_write(spec: &str, path: &str, mut data: impl Read) -> Result<(), String> {
    let dir = path.rsplit_once('/').map_or(".", |(d, _)| d);
    let cmd = format!("umask 077 && mkdir -p {dir} && cat > {path}");
    let mut child = Command::new("ssh")
        .args(crate::tmuxcc::ssh_args(spec))
        .arg(cmd)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "ssh did not start".to_string())?;
    if let Some(mut stdin) = child.stdin.take() {
        std::io::copy(&mut data, &mut stdin).map_err(|_| "copy to host failed".to_string())?;
    }
    match child.wait() {
        Ok(s) if s.success() => Ok(()),
        _ => Err("copy to host failed".into()),
    }
}

/// Ask for files with the desktop's picker (zenity, else kdialog). Blocks
/// until it closes: run it off the UI thread. Ok(empty) when cancelled.
pub fn pick_files() -> Result<Vec<PathBuf>, String> {
    let home = dirs::home_dir().unwrap_or_default();
    let tries: [(&str, Vec<String>); 2] = [
        ("zenity", vec!["--file-selection".into(), "--multiple".into(), "--separator=\n".into(), "--title=Upload to session".into()]),
        ("kdialog", vec!["--getopenfilename".into(), home.to_string_lossy().into_owned(), "--multiple".into(), "--separate-output".into()]),
    ];
    for (cmd, args) in tries {
        let Ok(out) = Command::new(cmd).args(&args).stderr(Stdio::null()).output() else { continue };
        let text = String::from_utf8_lossy(&out.stdout);
        return Ok(text.lines().filter(|l| !l.is_empty()).map(PathBuf::from).collect());
    }
    Err("install zenity or kdialog".into())
}

/// A local file's name, for toasts and remote copies.
pub fn file_name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "file".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(safe_name("my report (v2).pdf"), "my_report__v2_.pdf");
        assert_eq!(safe_name("..hidden"), "hidden");
        assert_eq!(safe_name("-rf"), "rf");
        assert_eq!(safe_name("ü"), "_");
        assert_eq!(safe_name(""), "file");
    }

    #[test]
    fn mentions() {
        assert_eq!(mention("/tmp/a.rs", true, false), "@/tmp/a.rs ");
        assert_eq!(mention("/tmp/a.png", true, true), "/tmp/a.png ");
        assert_eq!(mention("/tmp/a.rs", false, false), "/tmp/a.rs ");
        assert_eq!(mention("/tmp/it's here", false, false), r"'/tmp/it'\''s here' ");
        assert!(is_image("X.JPG") && !is_image("x.pdf") && !is_image("png"));
    }
}
