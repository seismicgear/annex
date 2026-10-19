//! Tauri command boundary for the native Array wallet transport.
//! Protocol and lifecycle logic live in the independently testable bridge.

pub(crate) use crate::array_wallet_bridge::ArrayWallet;
use crate::array_wallet_bridge::{
    PortableIdentity, WalletBackup, WalletError, WalletReceipt, WalletRecovery, WalletStatus,
};

async fn run<R: Send + 'static>(
    operation: impl FnOnce() -> Result<R, WalletError> + Send + 'static,
) -> Result<R, String> {
    tauri::async_runtime::spawn_blocking(operation)
        .await
        .map_err(|_| WalletError::Unavailable.message().to_owned())?
        .map_err(|error| error.message().to_owned())
}

#[tauri::command]
pub(crate) async fn array_wallet_status(
    wallet: tauri::State<'_, ArrayWallet>,
) -> Result<WalletStatus, String> {
    let wallet = wallet.inner().clone();
    run(move || Ok(wallet.status())).await
}

#[tauri::command]
pub(crate) async fn array_wallet_save_identity(
    value: PortableIdentity,
    wallet: tauri::State<'_, ArrayWallet>,
) -> Result<WalletReceipt, String> {
    let wallet = wallet.inner().clone();
    run(move || wallet.save(value)).await
}

#[tauri::command]
pub(crate) async fn array_wallet_list_backups(
    wallet: tauri::State<'_, ArrayWallet>,
) -> Result<Vec<WalletBackup>, String> {
    let wallet = wallet.inner().clone();
    run(move || wallet.list()).await
}

#[tauri::command]
pub(crate) async fn array_wallet_restore_identity(
    credential_id: String,
    wallet: tauri::State<'_, ArrayWallet>,
) -> Result<WalletRecovery, String> {
    let wallet = wallet.inner().clone();
    run(move || wallet.restore(credential_id)).await
}

#[tauri::command]
pub(crate) async fn array_wallet_disconnect(
    wallet: tauri::State<'_, ArrayWallet>,
) -> Result<WalletStatus, String> {
    let wallet = wallet.inner().clone();
    run(move || wallet.disconnect()).await
}
