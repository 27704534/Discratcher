//! Hand an image to the platform's burning tool and stream its progress.

use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const POLL: Duration = Duration::from_millis(100);
/// How long the tool gets to shut down before it is killed outright.
const TERMINATE_GRACE: Duration = Duration::from_secs(10);

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BurnProgress {
    pub percent: Option<f64>,
    pub message: String,
}

/// Parse one `hdiutil -puppetstrings` line.
pub fn parse_hdiutil_line(line: &str) -> Option<BurnProgress> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    // The man page says PERCENTAGE, hdiutil has been seen printing PERCENT: take either.
    if let Some(v) = line.strip_prefix("PERCENTAGE:").or_else(|| line.strip_prefix("PERCENT:")) {
        let p: f64 = v.trim().parse().ok()?;
        return Some(BurnProgress { percent: (p >= 0.0).then_some(p), message: String::new() });
    }
    let msg = line.strip_prefix("MESSAGE:").unwrap_or(line).trim();
    Some(BurnProgress { percent: None, message: msg.to_string() })
}

/// IMAPI_FORMAT2_DATA_WRITE_ACTION_FINALIZATION: from here on every sector is on the disc.
pub const ACTION_FINALIZING: i64 = 5;

/// Whether a burn that then reported an error had already put all its data on the disc.
/// Some drives fail the closing step (issue #9: "feature page not current") after a
/// complete write; that disc reads and measures fine, so the burn counts as done.
pub fn wrote_everything(furthest_action: i64, last_lba: i64, sectors: i64) -> bool {
    furthest_action >= ACTION_FINALIZING || (sectors > 0 && last_lba + 1 >= sectors)
}

/// Command line for the platforms that burn with an external tool. Windows burns in process
/// through IMAPI2 instead, so this is only reached from macOS and Linux. `erase` asks the
/// tool to erase a rewritable disc first; it comes from a drive the app listed as reusable.
pub fn burn_command(drive_id: &str, iso: &str, speed: u8, verify: bool, erase: bool) -> Result<Vec<String>, String> {
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = (drive_id, iso, speed, verify, erase);
        return Err("这个平台没有刻录后端".into());
    }
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    let _ = drive_id;
    #[cfg(target_os = "macos")]
    {
        let mut a = vec!["hdiutil".to_string(), "burn".into(), iso.into(), "-puppetstrings".into(), "-speed".into(), speed.to_string()];
        // Quick erase of a CD-RW, DVD-RW or DVD+RW: the only way a written rewritable
        // disc can be burned again. Nothing else in the app erases anything.
        if erase {
            a.push("-erase".into());
        }
        if !verify {
            a.push("-noverifyburn".into());
        }
        return Ok(a);
    }
    #[cfg(target_os = "linux")]
    {
        let mut a = vec!["growisofs".to_string(), "-dvd-compat".into(), format!("-speed={speed}")];
        // growisofs overwrites a DVD+RW by itself; nothing here erases anything yet.
        let _ = (verify, erase);
        a.push("-Z".into());
        a.push(format!("{drive_id}={iso}"));
        return Ok(a);
    }
    #[allow(unreachable_code)]
    Err("这个平台没有刻录后端".into())
}

/// Asks the tool to stop, so it can release the drive itself.
fn terminate(child: &mut Child) {
    #[cfg(unix)]
    let _ = Command::new("kill").args(["-TERM", &child.id().to_string()]).status();
    #[cfg(not(unix))]
    let _ = child.kill();
}

/// Blocks until the tool exits. Its own error text goes back unchanged: a refused drive
/// or medium is reported better there than anything guessed here. Setting `cancel` stops
/// the burn and returns `已取消`; a disc stopped mid-write is lost.
pub fn burn(argv: &[String], cancel: &AtomicBool, on_progress: &mut dyn FnMut(BurnProgress)) -> Result<(), String> {
    let mut child = Command::new(&argv[0])
        .args(&argv[1..])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("无法启动 {}：{e}", argv[0]))?;
    let stderr = child.stderr.take().unwrap();
    let err_thread = std::thread::spawn(move || {
        BufReader::new(stderr).lines().map_while(Result::ok).collect::<Vec<_>>().join("\n")
    });
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let mut last = String::new();
    let mut stopped_at: Option<Instant> = None;
    loop {
        match rx.recv_timeout(POLL) {
            Ok(line) => {
                if let Some(p) = parse_hdiutil_line(&line) {
                    if !p.message.is_empty() {
                        last = p.message.clone();
                    }
                    on_progress(p);
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        if cancel.load(Ordering::Relaxed) {
            match stopped_at {
                None => {
                    terminate(&mut child);
                    stopped_at = Some(Instant::now());
                }
                Some(t) if t.elapsed() > TERMINATE_GRACE => {
                    let _ = child.kill();
                    break;
                }
                Some(_) => {}
            }
        }
    }
    let status = child.wait().map_err(|e| e.to_string())?;
    if stopped_at.is_some() {
        return Err("已取消".into());
    }
    let err = err_thread.join().unwrap_or_default();
    if status.success() {
        Ok(())
    } else {
        Err(if err.trim().is_empty() {
        format!("刻录失败：{last}（退出码 {}）", status.code().unwrap_or(-1))
    } else {
        err
    })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_error_after_the_last_sector_is_not_a_failed_burn() {
        assert!(wrote_everything(5, 0, 0));
        assert!(wrote_everything(6, 100, 2_295_104));
        assert!(wrote_everything(4, 2_295_103, 2_295_104));
        assert!(!wrote_everything(4, 1_000_000, 2_295_104));
        assert!(!wrote_everything(-1, 0, 0));
    }

    #[test]
    fn parses_puppet_strings() {
        assert_eq!(parse_hdiutil_line("PERCENT:42.5").unwrap().percent, Some(42.5));
        assert_eq!(parse_hdiutil_line("PERCENTAGE:7.5").unwrap().percent, Some(7.5));
        assert_eq!(parse_hdiutil_line("PERCENT:-1.000000").unwrap().percent, None);
        assert_eq!(parse_hdiutil_line("MESSAGE:Burning").unwrap().message, "Burning");
        assert!(parse_hdiutil_line("   ").is_none());
    }

    #[test]
    fn a_rewritable_disc_is_erased_by_the_burner_itself() {
        if !cfg!(target_os = "macos") {
            return;
        }
        let plain = burn_command("disk8", "a.iso", 4, true, false).unwrap();
        assert!(!plain.iter().any(|a| a == "-erase"), "{plain:?}");
        let again = burn_command("disk8", "a.iso", 4, true, true).unwrap();
        assert!(again.iter().any(|a| a == "-erase"), "{again:?}");
        // Still one burn command: the erase is part of writing, not a step of its own.
        assert_eq!(again[0], "hdiutil");
        assert_eq!(again[1], "burn");
    }

    #[test]
    fn reports_tool_failure_text() {
        let argv = ["sh", "-c", "echo boom 1>&2; exit 3"].map(String::from);
        assert_eq!(burn(&argv, &AtomicBool::new(false), &mut |_| {}).unwrap_err(), "boom");
    }

    #[test]
    fn cancel_stops_the_tool_promptly() {
        let argv = ["sh", "-c", "echo PERCENT:10; exec sleep 30"].map(String::from);
        let cancel = AtomicBool::new(false);
        let t = Instant::now();
        let mut seen = None;
        let r = burn(&argv, &cancel, &mut |p| {
            seen = p.percent;
            cancel.store(true, Ordering::Relaxed);
        });
        assert_eq!(r.unwrap_err(), "已取消");
        assert_eq!(seen, Some(10.0));
        assert!(t.elapsed() < Duration::from_secs(5));
    }
}
