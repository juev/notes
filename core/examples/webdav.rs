//! A throwaway WebDAV server for trying sync between real devices:
//!
//! ```text
//! cargo run --example webdav -- /tmp/lists-dav 8765
//! ```
//!
//! Then point the apps at `http://<this machine>:8765/` with user `user` and
//! password `secret` (from the Android emulator the host is `10.0.2.2`).

#[path = "../tests/common/dav.rs"]
mod dav;

fn main() {
    let mut args = std::env::args().skip(1);
    let root = std::path::PathBuf::from(args.next().unwrap_or_else(|| "lists-dav".into()));
    let port = args.next().unwrap_or_else(|| "8765".into());
    std::fs::create_dir_all(&root).expect("create the folder");
    let server = tiny_http::Server::http(format!("0.0.0.0:{port}")).expect("bind the port");
    println!(
        "WebDAV on port {port}, files in {}, user: user, password: secret",
        root.display()
    );
    dav::serve(&server, &root);
}
