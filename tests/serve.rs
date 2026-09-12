use serde_json::Value;
use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    net::TcpStream,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

struct Server(Child);
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn get(address: &str, path: &str) -> (String, String) {
    let mut stream = TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    let (headers, body) = response.split_once("\r\n\r\n").unwrap();
    (headers.into(), body.into())
}
fn wait_status(address: &str, predicate: impl Fn(&Value) -> bool) -> Value {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let (_, body) = get(address, "/__slidown/status");
        let value: Value = serde_json::from_str(&body).unwrap();
        if predicate(&value) {
            return value;
        }
        assert!(
            Instant::now() < deadline,
            "preview did not reach expected state: {value}"
        );
        thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn preview_reloads_atomic_saves_images_and_recovers_from_errors() {
    let dir = tempfile::tempdir().unwrap();
    let source = "# Cover\n## Page\n![pic](pic.svg)\n";
    fs::write(dir.path().join("OUTLINE.md"), source).unwrap();
    fs::write(
        dir.path().join("pic.svg"),
        "<svg xmlns=\"http://www.w3.org/2000/svg\"/>",
    )
    .unwrap();
    let (mut server, address) = start(dir.path(), &["--output", "site"]);
    let address = address.as_str();
    let first = wait_status(address, |s| {
        s["revision"] != "initial" && s["error"].is_null()
    });
    let (headers, page) = get(address, "/");
    assert!(headers.contains("200 OK"));
    assert!(headers.to_lowercase().contains("cache-control: no-store"));
    assert!(page.contains("/__slidown/reload.js"));
    let disk = fs::read_to_string(dir.path().join("site/index.html")).unwrap();
    assert!(!disk.contains("__slidown"));
    fs::write(dir.path().join("site/private.txt"), "not public").unwrap();
    assert!(get(address, "/private.txt").0.contains("404"));
    assert!(get(address, "/%2e%2e/OUTLINE.md").0.contains("404"));
    let occupied = Command::new(env!("CARGO_BIN_EXE_slidown"))
        .current_dir(dir.path())
        .args(["serve", "--port", address.rsplit(':').next().unwrap()])
        .output()
        .unwrap();
    assert!(!occupied.status.success());

    fs::write(
        dir.path().join("pic.svg"),
        "<svg xmlns=\"http://www.w3.org/2000/svg\"><path/></svg>",
    )
    .unwrap();
    let second = wait_status(address, |s| {
        s["revision"] != first["revision"] && s["error"].is_null()
    });
    fs::write(
        dir.path().join("save.tmp"),
        "# Cover\n## Updated\n![pic](pic.svg)",
    )
    .unwrap();
    fs::rename(dir.path().join("save.tmp"), dir.path().join("OUTLINE.md")).unwrap();
    let third = wait_status(address, |s| {
        s["revision"] != second["revision"] && s["error"].is_null()
    });
    let good_disk = fs::read(dir.path().join("site/index.html")).unwrap();
    fs::write(
        dir.path().join("OUTLINE.md"),
        "# Cover\nForbidden cover body\n## Updated",
    )
    .unwrap();
    wait_status(address, |s| {
        s["error"]
            .as_str()
            .is_some_and(|e| e.contains("only blank lines"))
    });
    assert_eq!(
        fs::read(dir.path().join("site/index.html")).unwrap(),
        good_disk
    );
    assert!(get(address, "/").1.contains("Updated"));

    // Missing images discovered in a failed build must trigger recovery when created.
    fs::write(
        dir.path().join("OUTLINE.md"),
        "# Cover\n## Recovery\n![new](new.svg)",
    )
    .unwrap();
    wait_status(address, |s| {
        s["error"].as_str().is_some_and(|e| e.contains("new.svg"))
    });
    fs::write(
        dir.path().join("new.svg"),
        "<svg xmlns=\"http://www.w3.org/2000/svg\"/>",
    )
    .unwrap();
    wait_status(address, |s| {
        s["error"].is_null() && s["revision"] != third["revision"]
    });
    assert!(get(address, "/").1.contains("Recovery"));

    #[cfg(unix)]
    {
        stop(&mut server);
        assert!(!dir.path().join("site/index.html").exists());
        assert!(!dir.path().join("site/assets").exists());
        assert!(!dir.path().join("site").exists());
        assert!(dir.path().join("OUTLINE.md").exists());
        assert!(dir.path().join("pic.svg").exists());
        assert!(dir.path().join("new.svg").exists());
    }
}

fn start(root: &std::path::Path, args: &[&str]) -> (Server, String) {
    let mut server = Server(
        Command::new(env!("CARGO_BIN_EXE_slidown"))
            .current_dir(root)
            .args(["serve", "--port", "0"])
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let mut stdout = BufReader::new(server.0.stdout.take().unwrap());
    let mut line = String::new();
    stdout.read_line(&mut line).unwrap();
    assert!(
        line.starts_with("Preview: http://"),
        "server failed to start: {line}"
    );
    let address = line
        .trim()
        .strip_prefix("Preview: http://")
        .unwrap()
        .trim_end_matches('/')
        .to_owned();
    // Keep draining log output after the startup line so the child retains a reader.
    thread::spawn(move || {
        let _ = std::io::copy(&mut stdout, &mut std::io::sink());
    });
    (server, address)
}

#[cfg(unix)]
fn stop(server: &mut Server) {
    Command::new("kill")
        .args(["-INT", &server.0.id().to_string()])
        .status()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = server.0.try_wait().unwrap() {
            if !status.success() {
                let mut error = String::new();
                server
                    .0
                    .stderr
                    .as_mut()
                    .unwrap()
                    .read_to_string(&mut error)
                    .unwrap();
                panic!("serve exit failed: {status}: {error}");
            }
            return;
        }
        assert!(
            Instant::now() < deadline,
            "serve did not clean up and stop after SIGINT"
        );
        thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(unix)]
#[test]
fn serve_defaults_to_dist_and_removes_generated_directory_on_ctrl_c() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("OUTLINE.md"), "# Cover\n## Page").unwrap();
    let (mut server, address) = start(dir.path(), &[]);
    wait_status(&address, |s| {
        s["revision"] != "initial" && s["error"].is_null()
    });
    assert!(dir.path().join("dist/index.html").exists());
    assert!(!dir.path().join("index.html").exists());
    // An unsuccessful rebuild must still clean the last successful artifacts.
    fs::write(dir.path().join("OUTLINE.md"), "# Cover\nInvalid cover body").unwrap();
    wait_status(&address, |s| s["error"].is_string());
    stop(&mut server);
    assert!(!dir.path().join("dist").exists());
    assert!(dir.path().join("OUTLINE.md").exists());
}

#[cfg(unix)]
#[test]
fn cleanup_removes_the_entire_output_directory() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("OUTLINE.md"), "# Cover\n## Page").unwrap();
    let (mut server, address) = start(dir.path(), &[]);
    wait_status(&address, |s| {
        s["revision"] != "initial" && s["error"].is_null()
    });
    let output = dir.path().join("dist");
    let page = fs::read_to_string(output.join("index.html")).unwrap();
    assert!(page.contains("<style>"));
    assert!(page.contains("<script>"));
    assert!(page.contains("// Coalesce"));
    assert!(!output.join("assets").exists());
    fs::write(output.join("index.html"), "edited output").unwrap();
    fs::create_dir(output.join("extras")).unwrap();
    fs::write(output.join("extras/notes.txt"), "remove on exit").unwrap();
    stop(&mut server);
    assert!(!output.exists());
    assert!(dir.path().join("OUTLINE.md").exists());
}

#[cfg(unix)]
#[test]
fn no_successful_preview_leaves_previous_build_untouched() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("OUTLINE.md"), "# Original").unwrap();
    assert!(
        Command::new(env!("CARGO_BIN_EXE_slidown"))
            .current_dir(dir.path())
            .arg("build")
            .status()
            .unwrap()
            .success()
    );
    let original = fs::read(dir.path().join("dist/index.html")).unwrap();
    fs::write(dir.path().join("OUTLINE.md"), "# Original\nInvalid body").unwrap();
    let (mut server, address) = start(dir.path(), &[]);
    wait_status(&address, |s| s["error"].is_string());
    stop(&mut server);
    assert_eq!(
        fs::read(dir.path().join("dist/index.html")).unwrap(),
        original
    );
    assert!(!dir.path().join("dist/assets").exists());
}
