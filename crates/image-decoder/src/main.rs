#![forbid(unsafe_code)]

#[cfg(target_os = "linux")]
mod child;
#[cfg(target_os = "linux")]
mod sandbox;
#[cfg(target_os = "linux")]
mod server;
#[cfg(target_os = "linux")]
mod supervise;

#[cfg(target_os = "linux")]
fn main() {
    let args: Vec<_> = std::env::args_os().collect();
    let result = match args.as_slice() {
        [_, mode] if mode == "decode" => child::run(),
        [_, mode, socket, uid] if mode == "serve" => {
            let uid = uid.to_str().and_then(|v| v.parse::<u32>().ok());
            match uid {
                Some(uid) if uid != 0 && uid != rustix::process::getuid().as_raw() => {
                    tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .and_then(|rt| rt.block_on(server::serve(socket.into(), uid)))
                }
                _ => Err(std::io::Error::other("invalid peer identity")),
            }
        }
        _ => Err(std::io::Error::other("invalid invocation")),
    };
    if result.is_err() {
        eprintln!("image decoder unavailable");
        std::process::exit(1);
    }
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("image decoder requires Linux");
    std::process::exit(1);
}
