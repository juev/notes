//! A small command-line window into a Lists data folder, for debugging:
//!
//! ```text
//! cargo run --example lists -- <data folder> list
//! cargo run --example lists -- <data folder> add "call the bank tomorrow !!"
//! cargo run --example lists -- <data folder> note <task id> "text of the note"
//! cargo run --example lists -- <data folder> note <task id>
//! cargo run --example lists -- <data folder> webdav <url> <user>
//! LISTS_PASSWORD=… cargo run --example lists -- <data folder> sync
//! cargo run --example lists -- <data folder> sync-off
//! cargo run --example lists -- <data folder> import <backup.2dodb | todoist.csv | ticktick-backup.csv | omnifocus.csv | main.sqlite | trello.json | todo.json>
//! ```
//!
//! Close the app before pointing this at its folder.

use notes_core::{Scope, Store, SyncConfig};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let usage = "usage: lists <data folder> list | add <text> | note <task id> [text] | webdav <url> <user> | folder <path> | attach <task id> <file> | files <task id> | sync | sync-off | import <file>";
    let (dir, command) = match args.as_slice() {
        [dir, command, ..] => (dir.clone(), command.as_str()),
        _ => return Err(usage.into()),
    };
    let store = Store::open(dir)?;
    match (command, &args[2..]) {
        ("list", []) => {
            for task in store.tasks(Scope::All)? {
                println!("{}\t{}\t{}", task.list_id, task.due.unwrap_or_default(), task.title);
            }
        }
        ("add", [text]) => println!("{}", store.quick_add(text.clone(), None, false, None)?.id),
        ("note", [id, text]) => store.set_notes(id.clone(), text.clone())?,
        // Debug form, so that every character of the note is told from its neighbours.
        ("note", [id]) => println!("{:?}", store.task(id.clone())?.notes),
        ("webdav", [url, user]) => store.set_sync_config(SyncConfig::WebDav {
            url: url.clone(),
            user: user.clone(),
        })?,
        ("folder", [path]) => store.set_sync_config(SyncConfig::Folder { path: path.clone() })?,
        ("attach", [id, file]) => println!("{}", store.add_attachment(id.clone(), file.clone(), None)?.id),
        ("files", [id]) => {
            for file in store.attachments(id.clone())? {
                println!("{}\t{}\t{}", file.id, file.name, file.local_path.unwrap_or_default());
            }
            println!("waiting: {}", store.sync_status()?.attachments_waiting);
        }
        ("sync", []) => {
            // The password is never stored by the core; here it comes from the environment.
            store.set_sync_password(std::env::var("LISTS_PASSWORD").ok());
            println!("{:?}", store.sync_now()?);
            println!("{:?}", store.sync_attachments()?)
        }
        ("sync-off", []) => store.set_sync_config(SyncConfig::Off)?,
        ("import", [file]) => {
            let report = store.import_file(file.clone())?;
            println!(
                "{}: {} lists, {} tasks, {} attachments",
                report.source, report.lists, report.tasks, report.attachments
            );
            for note in report.notes {
                println!("- {note}");
            }
        }
        _ => return Err(usage.into()),
    }
    Ok(())
}
