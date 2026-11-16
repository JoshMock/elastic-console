use std::io::Write;
use std::process::{Command, Stdio};

/// `ELASTIC_CONSOLE_PAGER`, else `less -FRX` (-F exits at once when the text fits on one screen).
pub fn command() -> String {
    command_from(std::env::var("ELASTIC_CONSOLE_PAGER").ok())
}

fn command_from(var: Option<String>) -> String {
    var.filter(|v| !v.trim().is_empty()).unwrap_or_else(|| "less -FRX".to_string())
}

/// Pipes `text` through `sh -c cmd`. Writes it to `fallback` if the pager cannot start or exits non-zero.
pub fn page(cmd: &str, text: &str, fallback: &mut impl Write) {
    let child = Command::new("sh").args(["-c", cmd]).stdin(Stdio::piped()).spawn();
    let ok = match child {
        Ok(mut child) => {
            // ponytail: a write error means the user quit the pager early; ignore it
            let _ = child.stdin.take().map(|mut s| s.write_all(text.as_bytes()));
            child.wait().map(|s| s.success()).unwrap_or(false)
        }
        Err(_) => false,
    };
    if !ok {
        let _ = fallback.write_all(text.as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pipes_text_to_pager_command() {
        let out = std::env::temp_dir().join(format!("pager-pipe-test-{}", std::process::id()));
        let mut fb = Vec::new();
        page(&format!("cat > '{}'", out.display()), "{\n  \"a\": 1\n}\n", &mut fb);
        assert_eq!(std::fs::read_to_string(&out).unwrap(), "{\n  \"a\": 1\n}\n");
        assert!(fb.is_empty());
        let _ = std::fs::remove_file(out);
    }

    #[test]
    fn failing_pager_falls_back_to_direct_output() {
        let mut fb = Vec::new();
        page("cat >/dev/null; exit 1", "hello", &mut fb);
        assert_eq!(fb, b"hello");
    }

    #[test]
    fn blank_pager_uses_default() {
        assert_eq!(command_from(Some("  ".into())), "less -FRX");
        assert_eq!(command_from(None), "less -FRX");
        assert_eq!(command_from(Some("jless".into())), "jless");
    }
}
