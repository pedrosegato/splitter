use assert_cmd::Command;

fn help_output(args: &[&str]) -> String {
    let output = Command::cargo_bin("audiomirror-cli")
        .unwrap()
        .args(args)
        .output()
        .expect("run cli");
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    if !stdout.is_empty() {
        stdout
    } else {
        stderr
    }
}

#[test]
fn help_root() {
    let output = help_output(&["--help"]);
    insta::assert_snapshot!("help_root", output);
}

#[test]
fn help_devices() {
    let output = help_output(&["devices", "--help"]);
    insta::assert_snapshot!("help_devices", output);
}

#[test]
fn help_send() {
    let output = help_output(&["send", "--help"]);
    insta::assert_snapshot!("help_send", output);
}

#[test]
fn help_recv() {
    let output = help_output(&["recv", "--help"]);
    insta::assert_snapshot!("help_recv", output);
}

#[test]
fn help_loop() {
    let output = help_output(&["loop", "--help"]);
    insta::assert_snapshot!("help_loop", output);
}

#[test]
fn help_daemon() {
    let output = help_output(&["daemon", "--help"]);
    insta::assert_snapshot!("help_daemon", output);
}

#[test]
fn help_discover() {
    let output = help_output(&["discover", "--help"]);
    insta::assert_snapshot!("help_discover", output);
}

#[test]
fn help_stream_open() {
    let output = help_output(&["stream", "open", "--help"]);
    insta::assert_snapshot!("help_stream_open", output);
}

#[test]
fn help_stats() {
    let output = help_output(&["stats", "--help"]);
    insta::assert_snapshot!("help_stats", output);
}
