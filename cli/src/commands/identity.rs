use anyhow::Result;

use crate::book;

#[cfg(unix)]
use crate::commands::daemon::daemon_client;
use crate::util::*;

pub(crate) fn id() -> Result<()> {
    let id = my_identity()?;
    let pubid = id.public();
    println!("{}", encode_id(&pubid));
    eprintln!("fingerprint: {}", pubid.fingerprint());
    eprintln!("(identity stored at {})", identity_path().display());
    Ok(())
}

/// `arvolo name [NAME]` — show or set the local display name advertised in offers.
pub(crate) fn name_cmd(name: Option<String>) -> Result<()> {
    match name {
        None => {
            let current = book::my_display_name();
            if current.is_empty() {
                eprintln!("(no display name set — set one: arvolo name \"Your Name\")");
            } else {
                println!("{current}");
            }
        }
        Some(n) => {
            book::set_my_display_name(&n)?;
            let n = n.trim();
            if n.is_empty() {
                println!("Cleared your display name (offers will no longer advertise one).");
            } else {
                println!("Display name set to “{n}” — advertised inside offers you send.");
            }
        }
    }
    Ok(())
}

/// `arvolo version` — CLI version + whether a daemon is running (and its version).
pub(crate) async fn version_cmd() -> Result<()> {
    println!("arvolo {} (cli)", env!("CARGO_PKG_VERSION"));
    #[cfg(unix)]
    {
        match daemon_client().await {
            Some(mut c) => match c.status().await {
                Ok(st) => {
                    let ver = if st.version.is_empty() {
                        "unknown (older daemon — restart it to pick up this binary)".to_string()
                    } else {
                        format!("v{}", st.version)
                    };
                    println!(
                        "daemon:  running — {ver}  (relay {}, {} active, {} pending)",
                        st.relay.as_deref().unwrap_or("-"),
                        st.transfers,
                        st.pending
                    );
                }
                Err(e) => println!("daemon:  reachable but status failed: {e:#}"),
            },
            None => println!("daemon:  not running  (start it with `arvolo daemon`)"),
        }
    }
    #[cfg(not(unix))]
    println!("daemon:  not supported on this platform");
    Ok(())
}
