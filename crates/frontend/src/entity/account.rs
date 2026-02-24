use std::sync::Arc;

use bridge::account::Account;
use gpui::{App, Entity, EventEmitter};
use uuid::Uuid;

#[derive(Default)]
pub struct AccountEntries {
    pub accounts: Arc<[Account]>,
    pub selected_account_uuid: Option<Uuid>,
    pub selected_account: Option<Account>,
}

pub struct AccountChanged;

impl EventEmitter<AccountChanged> for AccountEntries {}

impl AccountEntries {
    pub fn set(entity: &Entity<Self>, accounts: Arc<[Account]>, selected_account: Option<Uuid>, cx: &mut App) {
        entity.update(cx, |entries, cx| {
            let prev_selected = entries.selected_account_uuid;
            entries.selected_account =
                selected_account.and_then(|uuid| accounts.iter().find(|acc| acc.uuid == uuid).cloned());
            entries.accounts = accounts;
            entries.selected_account_uuid = selected_account;
            cx.notify();
            if prev_selected != selected_account {
                cx.emit(AccountChanged);
            }
        });
    }
}
