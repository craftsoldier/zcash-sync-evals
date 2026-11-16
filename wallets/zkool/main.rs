//! Zkool driver.
//!
//! Built as an example inside Zkool's own `rust/` crate (hhanh00/zkool2), so it
//! compiles against Zkool's exact Cargo.lock and patches. It calls the same
//! functions the app and Zkool's GraphQL server use.
//!
//! The account is restored with the shielded pools only (Sapling, Orchard and
//! Ironwood). Zkool reads transparent history with GetTaddressTxids, which
//! lightwalletd turns into `getaddresstxids`, a question chain-index does not
//! answer. So unlike the other wallets, Zkool makes no transparent lookup here.
//!
//!   sync_eval sync <lightwalletd-url> <birthday> <mnemonic> <wallet-dir>
//!       Create the wallet, restore the account from the seed (`new_account`)
//!       and sync it to the tip, memo and detail pass included, then exit.
//!       The runner times this whole command.
//!
//!   sync_eval report <wallet-dir>
//!       Untimed. Print what the sync reached as JSON.

use std::path::Path;

use anyhow::{bail, Context as _, Result};
use rlz::api::account::{new_account, NewAccount};
use rlz::api::coin::Coin;
use rlz::api::sync::SyncOutcome;
use rlz::pay::pool::{POOL_IRONWOOD, POOL_ORCHARD, POOL_SAPLING};
use sqlx::{Connection as _, SqliteConnection};

/// Mainnet.
const COIN: u8 = 0;
/// What Zkool's server passes to a sync.
const ACTIONS_PER_SYNC: u32 = 100_000;

fn usage() -> ! {
    eprintln!(
        "usage: sync_eval sync <lightwalletd-url> <birthday> <mnemonic> <wallet-dir>\n       \
         sync_eval report <wallet-dir>"
    );
    std::process::exit(2);
}

#[tokio::main]
async fn main() -> Result<()> {
    rustls::crypto::ring::default_provider().install_default().ok();
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["sync", server, birthday, mnemonic, dir] => {
            sync(server, birthday.parse().context("birthday")?, mnemonic, Path::new(dir)).await
        }
        ["report", dir] => report(Path::new(dir)).await,
        _ => usage(),
    }
}

fn db_path(dir: &Path) -> String {
    dir.join("zkool.db").to_string_lossy().into_owned()
}

async fn open(dir: &Path) -> Result<Coin> {
    Coin::new(Some(COIN)).open_database(db_path(dir), None).await
}

async fn sync(server: &str, birthday: u32, mnemonic: &str, dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir).context("wallet dir")?;
    rlz::api::db::create_db(&db_path(dir), None, COIN).await?;
    let coin = open(dir).await?.set_lwd(0, server.into())?;
    let account = new_account(
        &NewAccount {
            icon: None,
            name: "eval".into(),
            restore: false,
            key: mnemonic.into(),
            passphrase: None,
            fingerprint: None,
            aindex: 0,
            birth: Some(birthday),
            folder: String::new(),
            pools: Some(POOL_SAPLING | POOL_ORCHARD | POOL_IRONWOOD),
            use_internal: Some(false),
            internal: false,
            hw: 0,
        },
        &coin,
    )
    .await?;
    let tip = rlz::api::network::get_current_height(&coin).await?;
    let timed = rlz::sync::bench::sync(&coin, vec![account], tip, ACTIONS_PER_SYNC, false).await?;
    match timed.outcome {
        SyncOutcome::Synced { height } => println!("synced to {height}"),
        other => bail!("the sync did not complete: {other:?}"),
    }
    println!("{}", serde_json::to_string(&timed.stats)?);
    // The report step needs the account; writing it is a few bytes.
    std::fs::write(dir.join("account"), account.to_string()).context("write account")?;
    Ok(())
}

async fn report(dir: &Path) -> Result<()> {
    let coin = open(dir).await?;
    let account: u32 = std::fs::read_to_string(dir.join("account"))
        .context("read account")?
        .parse()
        .context("account")?;
    let synced_height = rlz::api::sync::get_db_height(account, &coin).await?.height;
    let balance = rlz::api::sync::balance(account, &coin).await?.0;
    // A spent note is still a row of `notes`, and its transaction a row of `transactions`.
    let mut connection = SqliteConnection::connect(&format!("sqlite://{}", db_path(dir))).await?;
    let received_notes: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM notes WHERE account = ?1")
        .bind(account)
        .fetch_one(&mut connection)
        .await?;
    let history_entries: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM transactions WHERE account = ?1")
        .bind(account)
        .fetch_one(&mut connection)
        .await?;
    println!(
        "{}",
        serde_json::json!({
            "wallet": "zkool",
            "synced_height": synced_height,
            "received_notes": received_notes,
            "history_entries": history_entries,
            "balance": {
                "transparent": balance[0],
                "sapling": balance[1],
                "orchard": balance[2],
                "ironwood": balance[3],
                "total": balance.iter().sum::<u64>(),
            },
        })
    );
    Ok(())
}
