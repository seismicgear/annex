//! Native process contract driver; excluded from the Annex application.
//! Compiles the exact production bridge, including Windows GUI subsystem I/O.
#![allow(dead_code)]
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[path = "../../../crates/annex-desktop/src/array_wallet_bridge.rs"]
mod array_wallet_bridge;

use array_wallet_bridge::{ArrayWallet, PortableIdentity};
use serde_json::{json, Value};
use std::path::Path;

fn require(ok: bool, message: &str) -> Result<(), String> {
    if ok {
        Ok(())
    } else {
        Err(message.into())
    }
}

fn run(args: &[String]) -> Result<Value, String> {
    require(
        args.len() == 5,
        "Expected mode, profile, identity fixture and receipt",
    )?;
    let mode = &args[1];
    let profile = Path::new(&args[2]);
    std::fs::create_dir_all(profile).map_err(|_| "Cannot create isolated profile")?;
    let wallet = ArrayWallet::from_environment(profile);
    let status = serde_json::to_value(wallet.status()).map_err(|_| "Cannot encode status")?;
    if mode == "offline" {
        require(
            status["managedBy"] == "array" && status["available"] == false,
            "Offline launch lost its managed backup preference",
        )?;
        require(wallet.list().is_err(), "Offline backup must fail closed")?;
        return Ok(json!({"passed":true,"mode":mode,"retainedManagedPreference":true}));
    }
    require(
        status["managedBy"] == "array" && status["available"] == true && status["paired"] == true,
        "Actual Array broker did not establish pairing",
    )?;
    let expected: Value = serde_json::from_slice(
        &std::fs::read(&args[3]).map_err(|_| "Cannot read synthetic identity fixture")?,
    )
    .map_err(|_| "Invalid identity fixture")?;
    if mode == "save" {
        let identity: PortableIdentity =
            serde_json::from_value(expected.clone()).map_err(|_| "Invalid portable identity")?;
        wallet.save(identity).map_err(|error| error.message())?;
    } else {
        require(mode == "restore", "Unknown contract scenario")?;
    }
    let records = serde_json::to_value(wallet.list().map_err(|error| error.message())?)
        .map_err(|_| "Cannot encode metadata")?;
    require(
        records.as_array().is_some_and(|records| records.len() == 1),
        "Expected exactly one persisted identity",
    )?;
    let id = records[0]["id"].as_str().ok_or("Missing credential ID")?;
    let restored = serde_json::to_value(
        wallet
            .restore(id.to_owned())
            .map_err(|error| error.message())?,
    )
    .map_err(|_| "Cannot encode restored fixture")?;
    require(
        restored["value"] == expected && restored["managedBy"] == "array",
        "Restored identity differs from the original",
    )?;
    if mode == "restore" {
        let result = serde_json::to_value(wallet.disconnect().map_err(|error| error.message())?)
            .map_err(|_| "Cannot encode disconnect receipt")?;
        require(
            result["managedBy"].is_null() && !profile.join("array-wallet-managed").exists(),
            "Disconnect did not clear local preference",
        )?;
    }
    Ok(
        json!({"passed":true,"mode":mode,"restoredExactIdentity":true,"disconnected":mode=="restore"}),
    )
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let result = run(&args);
    let receipt = match &result {
        Ok(value) => value.clone(),
        Err(message) => json!({"passed":false,"error":message}),
    };
    if let Some(output) = args.get(4) {
        if std::fs::write(output, serde_json::to_vec_pretty(&receipt).unwrap()).is_err() {
            std::process::exit(2);
        }
    }
    if result.is_err() {
        std::process::exit(1);
    }
}
