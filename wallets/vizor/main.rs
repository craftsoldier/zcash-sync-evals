//! Vizor driver.
//!
//! Built as an example inside Vizor's own `rust/` crate (chainapsis/vizor-wallet),
//! so it compiles against Vizor's exact Cargo.lock and patches. It calls the same
//! public API the app and Vizor's own integration tests use.
//!
//! Private queries (PIR) stay off, Vizor's default: lightwalletd only.
//!
//!   sync_eval sync <lightwalletd-url> <birthday> <mnemonic> <wallet-dir>
//!       Restore from the seed (`import_wallet`) and sync to the tip
//!       (`run_full_sync_blocking`), then exit. The runner times this whole command.
//!
//!   sync_eval report <wallet-dir>
//!       Untimed. Print what the sync reached as JSON.

use std::path::Path;

use rust_lib_zcash_wallet::api::{sync as sync_api, wallet as wallet_api};

fn usage() -> ! {
    eprintln!(
        "usage: sync_eval sync <lightwalletd-url> <birthday> <mnemonic> <wallet-dir>\n       \
         sync_eval report <wallet-dir>"
    );
    std::process::exit(2);
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["sync", server, birthday, mnemonic, dir] => sync(
            server,
            birthday.parse().expect("birthday"),
            mnemonic,
            Path::new(dir),
        ),
        ["report", dir] => report(Path::new(dir)),
        _ => usage(),
    }
}

fn db_path(dir: &Path) -> String {
    dir.join("zcash_wallet.db").to_string_lossy().into_owned()
}

fn sync(server: &str, birthday: u64, mnemonic: &str, dir: &Path) {
    std::fs::create_dir_all(dir).expect("wallet dir");
    let db = db_path(dir);
    sync_api::set_enhance_pir_enabled(false);
    let wallet = wallet_api::import_wallet(
        mnemonic.into(),
        String::new(),
        Some(birthday),
        "main".into(),
        db.clone(),
        None,
    )
    .expect("import_wallet");
    sync_api::run_full_sync_blocking(db, server.into(), "main".into(), 1)
        .expect("run_full_sync_blocking");
    // The report step needs the account; writing it is a few bytes.
    std::fs::write(dir.join("account"), &wallet.account_uuid).expect("write account");
}

fn report(dir: &Path) {
    let db = db_path(dir);
    let account = std::fs::read_to_string(dir.join("account")).expect("read account");
    let b = sync_api::get_balance(db.clone(), "main".into(), account.clone()).expect("get_balance");
    let status = sync_api::get_sync_status(db.clone(), "main".into()).expect("get_sync_status");
    // A spent note still counts as wallet history.
    let history_entries =
        sync_api::get_transaction_history(db.clone(), "main".into(), None, account)
            .expect("get_transaction_history")
            .len();
    let conn = rusqlite::Connection::open(&db).expect("open wallet for note counts");
    let tables: Vec<String> = conn
        .prepare(
            "SELECT name FROM sqlite_master WHERE type='table' AND name LIKE '%received_notes'",
        )
        .expect("note tables")
        .query_map([], |r| r.get(0))
        .expect("query tables")
        .collect::<Result<_, _>>()
        .expect("table names");
    assert!(!tables.is_empty(), "no received-note tables found");
    let received_notes: i64 = tables
        .iter()
        .map(|name| {
            let sql = format!("SELECT COUNT(*) FROM \"{}\"", name.replace('"', "\"\""));
            conn.query_row(&sql, [], |r| r.get::<_, i64>(0))
                .expect("note count")
        })
        .sum();
    println!(
        "{{\"wallet\":\"vizor\",\"synced_height\":{},\"chain_tip\":{},\"received_notes\":{received_notes},\
         \"history_entries\":{history_entries},\"balance\":{{\"transparent\":{},\"sapling\":{},\"orchard\":{},\
         \"ironwood\":{},\"total\":{}}}}}",
        status.scanned_height, status.chain_tip_height, b.transparent, b.sapling, b.orchard, b.ironwood, b.total
    );
}
