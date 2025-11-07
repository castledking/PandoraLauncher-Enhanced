use std::sync::Arc;

use bridge::account::Account;
use gpui::{AppContext, Entity};
use uuid::Uuid;

#[derive(Default)]
pub struct AccountEntries {
    pub accounts: Arc<[Account]>,
    pub selected_account_uuid: Option<Uuid>,
    pub selected_account: Option<Account>,
}

impl AccountEntries {
    pub fn set<C: AppContext>(entity: &Entity<Self>, accounts: Arc<[Account]>, selected_account: Option<Uuid>, cx: &mut C) {
        entity.update(cx, |entries, cx| {
            entries.selected_account = selected_account.and_then(|uuid| accounts.iter().find(|acc| acc.uuid == uuid).map(Account::clone));
            entries.accounts = accounts;
            entries.selected_account_uuid = selected_account;
            cx.notify();
        });
    }
}
