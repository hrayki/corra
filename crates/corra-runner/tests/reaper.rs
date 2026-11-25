#![cfg(target_os = "linux")]

use std::process::Command;

const NAME: &str = "an_orphan_of_the_runners_processes_is_adopted_and_collected";
const SCRIPT: &str = r#"
(sleep 0.3 & echo $! > "$ORPHAN")
sleep 0.1
o=$(cat "$ORPHAN")
[ "$(awk '{print $4}' /proc/$o/stat)" = "$PPID" ] || exit 4
sleep 0.7
[ -e /proc/$o ] && exit 3
exit 0
"#;

#[test]
fn an_orphan_of_the_runners_processes_is_adopted_and_collected() {
    if let Some(file) = std::env::var_os("CORRA_REAPER_TEST_ORPHAN") {
        let mut cmd = Command::new("bash");
        cmd.args(["-c", SCRIPT]).env("ORPHAN", file);
        std::process::exit(corra_runner::reaper::supervise(cmd).unwrap());
    }
    let dir = tempfile::tempdir().unwrap();
    let status = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", NAME, "--nocapture", "--test-threads=1"])
        .env("CORRA_REAPER_TEST_ORPHAN", dir.path().join("orphan"))
        .status()
        .unwrap();
    assert_eq!(
        status.code(),
        Some(0),
        "4: the orphan went to init, not the runner; 3: it was left a zombie"
    );
}
