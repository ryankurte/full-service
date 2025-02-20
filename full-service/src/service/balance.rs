// Copyright (c) 2020-2021 MobileCoin Inc.

//! Service for managing balances.

use crate::{
    db::{
        account::{AccountID, AccountModel},
        assigned_subaddress::AssignedSubaddressModel,
        models::{
            Account, AssignedSubaddress, Txo, ViewOnlyAccount, ViewOnlySubaddress, ViewOnlyTxo,
        },
        txo::TxoModel,
        view_only_account::ViewOnlyAccountModel,
        view_only_subaddress::ViewOnlySubaddressModel,
        view_only_txo::ViewOnlyTxoModel,
        Conn, WalletDbError,
    },
    service::{
        ledger::{LedgerService, LedgerServiceError},
        WalletService,
    },
};
use displaydoc::Display;
use mc_common::HashMap;
use mc_connection::{BlockchainConnection, UserTxConnection};
use mc_fog_report_validation::FogPubkeyResolver;
use mc_ledger_db::Ledger;

/// Errors for the Address Service.
#[derive(Display, Debug)]
#[allow(clippy::large_enum_variant)]
pub enum BalanceServiceError {
    /// Error interacting with the database: {0}
    Database(WalletDbError),

    /// Diesel Error: {0}
    Diesel(diesel::result::Error),

    /// Error with LedgerDB: {0}
    LedgerDB(mc_ledger_db::Error),

    /// Error getting network block height: {0}
    NetworkBlockHeight(LedgerServiceError),

    /// Unexpected Account Txo Status: {0}
    UnexpectedAccountTxoStatus(String),
}

impl From<WalletDbError> for BalanceServiceError {
    fn from(src: WalletDbError) -> Self {
        Self::Database(src)
    }
}

impl From<diesel::result::Error> for BalanceServiceError {
    fn from(src: diesel::result::Error) -> Self {
        Self::Diesel(src)
    }
}

impl From<mc_ledger_db::Error> for BalanceServiceError {
    fn from(src: mc_ledger_db::Error) -> Self {
        Self::LedgerDB(src)
    }
}

impl From<LedgerServiceError> for BalanceServiceError {
    fn from(src: LedgerServiceError) -> Self {
        Self::NetworkBlockHeight(src)
    }
}

/// The balance object returned by balance services.
///
/// This must be a service object because there is no "Balance" table in our
/// data model.
pub struct Balance {
    pub unspent: u128,
    pub pending: u128,
    pub spent: u128,
    pub secreted: u128,
    pub orphaned: u128,
    pub network_block_height: u64,
    pub local_block_height: u64,
    pub synced_blocks: u64,
    pub max_spendable: u128,
}

/// The Network Status object.
/// This holds the number of blocks in the ledger, on the network and locally.
pub struct NetworkStatus {
    pub network_block_height: u64,
    pub local_block_height: u64,
    pub fee_pmob: u64,
    pub block_version: u32,
}

/// The Wallet Status object returned by balance services.
///
/// This must be a service object because there is no "WalletStatus" table in
/// our data model.
///
/// It shares several fields with balance, but also returns details about the
/// accounts in the wallet.
pub struct WalletStatus {
    pub unspent: u128,
    pub pending: u128,
    pub spent: u128,
    pub secreted: u128,
    pub orphaned: u128,
    pub network_block_height: u64,
    pub local_block_height: u64,
    pub min_synced_block_index: u64,
    pub account_ids: Vec<AccountID>,
    pub account_map: HashMap<AccountID, Account>,
    pub view_only_account_ids: Vec<String>,
    pub view_only_account_map: HashMap<String, ViewOnlyAccount>,
}

/// Trait defining the ways in which the wallet can interact with and manage
/// balances.
pub trait BalanceService {
    /// Gets the balance for a given account.
    ///
    /// Balance consists of the sums of the various txo states in our wallet
    fn get_balance_for_account(
        &self,
        account_id: &AccountID,
    ) -> Result<Balance, BalanceServiceError>;

    fn get_balance_for_view_only_account(
        &self,
        account_id: &str,
    ) -> Result<Balance, BalanceServiceError>;

    fn get_balance_for_address(&self, address: &str) -> Result<Balance, BalanceServiceError>;

    fn get_balance_for_view_only_address(
        &self,
        address: &str,
    ) -> Result<Balance, BalanceServiceError>;

    fn get_network_status(&self) -> Result<NetworkStatus, BalanceServiceError>;

    fn get_wallet_status(&self) -> Result<WalletStatus, BalanceServiceError>;
}

impl<T, FPR> BalanceService for WalletService<T, FPR>
where
    T: BlockchainConnection + UserTxConnection + 'static,
    FPR: FogPubkeyResolver + Send + Sync + 'static,
{
    fn get_balance_for_account(
        &self,
        account_id: &AccountID,
    ) -> Result<Balance, BalanceServiceError> {
        let account_id_hex = &account_id.to_string();

        let conn = self.wallet_db.get_conn()?;
        let (unspent, max_spendable, pending, spent, secreted, orphaned) =
            Self::get_balance_inner(account_id_hex, None, &conn)?;

        let network_block_height = self.get_network_block_height()?;
        let local_block_height = self.ledger_db.num_blocks()?;
        let account = Account::get(account_id, &conn)?;

        Ok(Balance {
            unspent,
            max_spendable,
            pending,
            spent,
            secreted,
            orphaned,
            network_block_height,
            local_block_height,
            synced_blocks: account.next_block_index as u64,
        })
    }

    fn get_balance_for_view_only_account(
        &self,
        account_id: &str,
    ) -> Result<Balance, BalanceServiceError> {
        let conn = self.wallet_db.get_conn()?;

        let (unspent, max_spendable, pending, spent, secreted, orphaned) =
            Self::get_view_only_balance_inner(account_id, None, &conn)?;

        let network_block_height = self.get_network_block_height()?;
        let local_block_height = self.ledger_db.num_blocks()?;
        let account = ViewOnlyAccount::get(account_id, &conn)?;

        Ok(Balance {
            unspent,
            pending,
            spent,
            secreted,
            orphaned,
            network_block_height,
            local_block_height,
            synced_blocks: account.next_block_index as u64,
            max_spendable,
        })
    }

    fn get_balance_for_address(&self, address: &str) -> Result<Balance, BalanceServiceError> {
        let network_block_height = self.get_network_block_height()?;
        let local_block_height = self.ledger_db.num_blocks()?;

        let conn = self.wallet_db.get_conn()?;
        let assigned_address = AssignedSubaddress::get(address, &conn)?;

        let (unspent, max_spendable, pending, spent, secreted, orphaned) =
            Self::get_balance_inner(&assigned_address.account_id_hex, Some(address), &conn)?;

        let account = Account::get(&AccountID(assigned_address.account_id_hex), &conn)?;

        Ok(Balance {
            unspent,
            max_spendable,
            pending,
            spent,
            secreted,
            orphaned,
            network_block_height,
            local_block_height,
            synced_blocks: account.next_block_index as u64,
        })
    }

    fn get_balance_for_view_only_address(
        &self,
        address: &str,
    ) -> Result<Balance, BalanceServiceError> {
        let conn = self.wallet_db.get_conn()?;
        let view_only_subaddress = ViewOnlySubaddress::get(address, &conn)?;
        let (unspent, max_spendable, pending, spent, secreted, orphaned) =
            Self::get_view_only_balance_inner(
                &view_only_subaddress.view_only_account_id_hex,
                Some(address),
                &conn,
            )?;

        let network_block_height = self.get_network_block_height()?;
        let local_block_height = self.ledger_db.num_blocks()?;
        let account = ViewOnlyAccount::get(&view_only_subaddress.view_only_account_id_hex, &conn)?;

        Ok(Balance {
            unspent,
            max_spendable,
            pending,
            spent,
            secreted,
            orphaned,
            network_block_height,
            local_block_height,
            synced_blocks: account.next_block_index as u64,
        })
    }
    fn get_network_status(&self) -> Result<NetworkStatus, BalanceServiceError> {
        Ok(NetworkStatus {
            network_block_height: self.get_network_block_height()?,
            local_block_height: self.ledger_db.num_blocks()?,
            fee_pmob: self.get_network_fee(),
            block_version: *self.get_network_block_version(),
        })
    }

    // Wallet Status is an overview of the wallet's status
    fn get_wallet_status(&self) -> Result<WalletStatus, BalanceServiceError> {
        let network_block_height = self.get_network_block_height()?;

        let conn = self.wallet_db.get_conn()?;
        let accounts = Account::list_all(&conn)?;
        let mut account_map = HashMap::default();
        let view_only_accounts = ViewOnlyAccount::list_all(&conn)?;
        let mut view_only_account_map = HashMap::default();

        let mut unspent: u128 = 0;
        let mut pending: u128 = 0;
        let mut spent: u128 = 0;
        let mut secreted: u128 = 0;
        let mut orphaned: u128 = 0;

        let mut min_synced_block_index = network_block_height - 1;
        let mut account_ids = Vec::new();

        for account in accounts {
            let account_id = AccountID(account.account_id_hex.clone());
            let balance = Self::get_balance_inner(&account_id.to_string(), None, &conn)?;
            account_map.insert(account_id.clone(), account.clone());
            unspent += balance.0;
            pending += balance.2;
            spent += balance.3;
            secreted += balance.4;
            orphaned += balance.5;

            // account.next_block_index is an index in range [0..ledger_db.num_blocks()]
            min_synced_block_index = std::cmp::min(
                min_synced_block_index,
                (account.next_block_index as u64).saturating_sub(1),
            );
            account_ids.push(account_id);
        }

        let mut view_only_account_ids = Vec::new();
        for account in view_only_accounts {
            let account_id = account.account_id_hex.clone();
            view_only_account_map.insert(account_id.clone(), account.clone());
            view_only_account_ids.push(account_id);
        }

        Ok(WalletStatus {
            unspent,
            pending,
            spent,
            secreted,
            orphaned,
            network_block_height,
            local_block_height: self.ledger_db.num_blocks()?,
            min_synced_block_index: min_synced_block_index as u64,
            account_ids,
            account_map,
            view_only_account_ids,
            view_only_account_map,
        })
    }
}

impl<T, FPR> WalletService<T, FPR>
where
    T: BlockchainConnection + UserTxConnection + 'static,
    FPR: FogPubkeyResolver + Send + Sync + 'static,
{
    fn get_balance_inner(
        account_id_hex: &str,
        assigned_subaddress_b58: Option<&str>,
        conn: &Conn,
    ) -> Result<(u128, u128, u128, u128, u128, u128), BalanceServiceError> {
        let max_spendable =
            Txo::list_spendable(account_id_hex, None, assigned_subaddress_b58, Some(0), conn)?
                .max_spendable_in_wallet;
        let unspent = Txo::list_unspent(account_id_hex, assigned_subaddress_b58, Some(0), conn)?
            .iter()
            .map(|t| (t.value as u64) as u128)
            .sum::<u128>();
        let spent = Txo::list_spent(account_id_hex, assigned_subaddress_b58, Some(0), conn)?
            .iter()
            .map(|t| (t.value as u64) as u128)
            .sum::<u128>();
        let pending = Txo::list_pending(account_id_hex, assigned_subaddress_b58, Some(0), conn)?
            .iter()
            .map(|t| (t.value as u64) as u128)
            .sum::<u128>();

        let secreted = if assigned_subaddress_b58.is_some() {
            0
        } else {
            Txo::list_secreted(account_id_hex, Some(0), conn)?
                .iter()
                .map(|t| t.value as u128)
                .sum::<u128>()
        };

        let orphaned = if assigned_subaddress_b58.is_some() {
            0
        } else {
            Txo::list_orphaned(account_id_hex, Some(0), conn)?
                .iter()
                .map(|t| t.value as u128)
                .sum::<u128>()
        };

        let result = (unspent, max_spendable, pending, spent, secreted, orphaned);
        Ok(result)
    }

    fn get_view_only_balance_inner(
        account_id_hex: &str,
        assigned_subaddress_b58: Option<&str>,
        conn: &Conn,
    ) -> Result<(u128, u128, u128, u128, u128, u128), BalanceServiceError> {
        let unspent =
            ViewOnlyTxo::list_unspent(account_id_hex, assigned_subaddress_b58, Some(0), conn)?
                .iter()
                .map(|t| (t.value as u64) as u128)
                .sum::<u128>();
        let spent =
            ViewOnlyTxo::list_spent(account_id_hex, assigned_subaddress_b58, Some(0), conn)?
                .iter()
                .map(|t| (t.value as u64) as u128)
                .sum::<u128>();
        let orphaned = ViewOnlyTxo::list_orphaned(account_id_hex, Some(0), conn)?
            .iter()
            .map(|t| (t.value as u64) as u128)
            .sum::<u128>();
        let pending =
            ViewOnlyTxo::list_pending(account_id_hex, assigned_subaddress_b58, Some(0), conn)?
                .iter()
                .map(|t| (t.value as u64) as u128)
                .sum::<u128>();

        let result = (unspent, 0, pending, spent, 0, orphaned);
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        service::{
            account::AccountService, address::AddressService,
            view_only_account::ViewOnlyAccountService,
        },
        test_utils::{get_test_ledger, manually_sync_account, setup_wallet_service, MOB},
        util::b58::b58_encode_public_address,
    };
    use mc_account_keys::{
        AccountKey, PublicAddress, RootEntropy, RootIdentity, CHANGE_SUBADDRESS_INDEX,
        DEFAULT_SUBADDRESS_INDEX,
    };
    use mc_common::logger::{test_with_logger, Logger};
    use mc_crypto_keys::{RistrettoPrivate, RistrettoPublic};
    use mc_transaction_core::{
        encrypted_fog_hint::EncryptedFogHint, tokens::Mob, tx::TxOut, Amount, Token,
    };
    use mc_util_from_random::FromRandom;
    use rand::{rngs::StdRng, SeedableRng};

    // The balance for an address should be accurate.
    #[test_with_logger]
    fn test_address_balance(logger: Logger) {
        let mut rng: StdRng = SeedableRng::from_seed([20u8; 32]);

        let entropy = RootEntropy::from_random(&mut rng);
        let account_key = AccountKey::from(&RootIdentity::from(&entropy));

        // Set up the ledger to be seeded with multiple subaddresses paid
        let public_address0 = account_key.subaddress(0);
        let public_address1 = account_key.subaddress(1);
        let public_address2 = account_key.subaddress(2);
        let public_address3 = account_key.subaddress(3);

        let known_recipients: Vec<PublicAddress> = vec![
            public_address0.clone(),
            public_address1,
            public_address2,
            public_address3.clone(),
        ];
        let ledger_db = get_test_ledger(5, &known_recipients, 12, &mut rng);

        let service = setup_wallet_service(ledger_db.clone(), logger.clone());

        let account = service
            .import_account_from_legacy_root_entropy(
                hex::encode(&entropy.bytes),
                None,
                None,
                None,
                "".to_string(),
                "".to_string(),
                "".to_string(),
            )
            .expect("Could not import account entropy");

        let address = service
            .assign_address_for_account(&AccountID(account.account_id_hex.clone()), None)
            .expect("Could not assign address");
        assert_eq!(address.subaddress_index, 2);

        let _account = manually_sync_account(
            &ledger_db,
            &service.wallet_db,
            &AccountID(account.account_id_hex.to_string()),
            &logger,
        );

        let account_balance = service
            .get_balance_for_account(&AccountID(account.account_id_hex))
            .expect("Could not get balance for account");

        // 3 accounts * 5_000 MOB * 12 blocks
        assert_eq!(account_balance.unspent, 180_000 * MOB as u128);
        // 5_000 MOB per txo, max 16 txos input - network fee
        assert_eq!(account_balance.max_spendable, 79999999600000000 as u128);
        assert_eq!(account_balance.pending, 0);
        assert_eq!(account_balance.spent, 0);
        assert_eq!(account_balance.secreted, 0);
        assert_eq!(account_balance.orphaned, 60_000 * MOB as u128); // Public address 3

        let db_account_key: AccountKey =
            mc_util_serial::decode(&account.account_key).expect("Could not decode account key");
        let db_pub_address = db_account_key.subaddress(account.main_subaddress_index as u64);
        assert_eq!(db_pub_address, public_address0);
        let b58_pub_address =
            b58_encode_public_address(&db_pub_address).expect("Could not encode public address");
        let address_balance = service
            .get_balance_for_address(&b58_pub_address)
            .expect("Could not get balance for address");

        assert_eq!(address_balance.unspent, 60_000 * MOB as u128);
        assert_eq!(address_balance.max_spendable, 59999999600000000 as u128);
        assert_eq!(address_balance.pending, 0);
        assert_eq!(address_balance.spent, 0);
        assert_eq!(address_balance.secreted, 0);
        assert_eq!(address_balance.orphaned, 0);

        let address_balance2 = service
            .get_balance_for_address(&address.assigned_subaddress_b58)
            .expect("Could not get balance for address");
        assert_eq!(address_balance2.unspent, 60_000 * MOB as u128);
        assert_eq!(address_balance2.max_spendable, 59999999600000000 as u128);
        assert_eq!(address_balance2.pending, 0);
        assert_eq!(address_balance2.spent, 0);
        assert_eq!(address_balance2.secreted, 0);
        assert_eq!(address_balance2.orphaned, 0);

        // Even though subaddress 3 has funds, we are not watching it, so we should get
        // an error.
        let b58_pub_address3 =
            b58_encode_public_address(&public_address3).expect("Could not encode public address");
        match service.get_balance_for_address(&b58_pub_address3) {
            Ok(_) => panic!("Should not get success getting balance for a non-assigned address"),
            Err(BalanceServiceError::Database(WalletDbError::AssignedSubaddressNotFound(_))) => {}
            Err(e) => panic!("Unexpected error {:?}", e),
        }
    }

    // The balance for an address should be accurate.
    #[test_with_logger]
    fn test_view_only_balance(logger: Logger) {
        // setup view only account
        let mut rng: StdRng = SeedableRng::from_seed([20u8; 32]);
        let known_recipients: Vec<PublicAddress> = Vec::new();
        let current_block_height = 12; //index 11
        let ledger_db = get_test_ledger(
            5,
            &known_recipients,
            current_block_height as usize,
            &mut rng,
        );
        let service = setup_wallet_service(ledger_db.clone(), logger.clone());
        let conn = service.wallet_db.get_conn().unwrap();

        let view_private_key = RistrettoPrivate::from_random(&mut rng);
        let spend_private_key = RistrettoPrivate::from_random(&mut rng);

        let name = "testing";

        let account_key = AccountKey::new(&spend_private_key, &view_private_key);
        let account_id = AccountID::from(&account_key);
        let main_public_address = account_key.default_subaddress();
        let change_public_address = account_key.change_subaddress();
        let mut subaddresses: Vec<(String, u64, String, RistrettoPublic)> = Vec::new();
        subaddresses.push((
            b58_encode_public_address(&main_public_address).unwrap(),
            DEFAULT_SUBADDRESS_INDEX,
            "Main".to_string(),
            *main_public_address.spend_public_key(),
        ));
        subaddresses.push((
            b58_encode_public_address(&change_public_address).unwrap(),
            CHANGE_SUBADDRESS_INDEX,
            "Change".to_string(),
            *change_public_address.spend_public_key(),
        ));

        service
            .import_view_only_account(
                &account_id.to_string(),
                &view_private_key,
                DEFAULT_SUBADDRESS_INDEX,
                CHANGE_SUBADDRESS_INDEX,
                2,
                name.clone(),
                subaddresses,
            )
            .unwrap();

        // add funds to account
        for _ in 0..2 {
            let value = 420 * MOB;
            let amount = Amount::new(value, Mob::ID);
            let tx_private_key = RistrettoPrivate::from_random(&mut rng);
            let hint = EncryptedFogHint::fake_onetime_hint(&mut rng);
            let fake_tx_out =
                TxOut::new(amount, &main_public_address, &tx_private_key, hint).unwrap();
            ViewOnlyTxo::create(
                fake_tx_out.clone(),
                amount,
                Some(DEFAULT_SUBADDRESS_INDEX),
                Some(current_block_height),
                &account_id.to_string(),
                &conn,
            )
            .unwrap();
        }

        // test balance for account
        let balance: Balance = service
            .get_balance_for_view_only_account(&account_id.to_string())
            .unwrap();
        assert_eq!(balance.unspent as u64, 840 * MOB);
        // view only accounts have no spendable MOB
        assert_eq!(balance.max_spendable, 0);
        assert_eq!(balance.spent, 0);
        assert_eq!(balance.pending, 0);
        assert_eq!(balance.secreted, 0);
        assert_eq!(balance.orphaned, 0);

        // add funds to specific address
        let subaddress_index = 3;
        let subaddress = account_key.subaddress(subaddress_index);
        let b58_pub_address =
            b58_encode_public_address(&subaddress).expect("Could not encode public address");
        service
            .import_subaddresses(
                &account_id.to_string(),
                [(
                    b58_pub_address.clone(),
                    subaddress_index,
                    "cheese".to_string(),
                    subaddress.spend_public_key().to_owned(),
                )]
                .to_vec(),
            )
            .unwrap();

        let value = 100 * MOB;
        let amount = Amount::new(value, Mob::ID);
        let tx_private_key = RistrettoPrivate::from_random(&mut rng);
        let hint = EncryptedFogHint::fake_onetime_hint(&mut rng);
        let fake_tx_out = TxOut::new(amount, &main_public_address, &tx_private_key, hint).unwrap();
        ViewOnlyTxo::create(
            fake_tx_out.clone(),
            amount,
            Some(subaddress_index),
            Some(current_block_height),
            &account_id.to_string(),
            &conn,
        )
        .unwrap();

        let balance: Balance = service
            .get_balance_for_view_only_address(&b58_pub_address)
            .unwrap();
        assert_eq!(balance.unspent as u64, 100 * MOB);
        // view only accounts have no spendable MOB
        assert_eq!(balance.max_spendable, 0);
        assert_eq!(balance.spent, 0);
        assert_eq!(balance.pending, 0);
        assert_eq!(balance.secreted, 0);
        assert_eq!(balance.orphaned, 0);
    }
}
