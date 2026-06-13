use anyhow::Result;
use num_bigint::BigUint;
use num_traits::Zero;
use ul_crypto::Keypair;
use ul_ledger::Ledger;
use ul_types::{AccountId, Amount, BlockCapacity, SignedTx, TOTAL_SUPPLY, TxKind};

fn sum_balances(snapshot: &ul_ledger::StateSnapshot) -> BigUint {
    snapshot
        .balances
        .values()
        .fold(BigUint::zero(), |acc, amount| acc + &amount.0)
}

fn signed_transfer(kp: &Keypair, to: AccountId, amount: Amount, nonce: u64) -> SignedTx {
    let mut tx = SignedTx {
        from: kp.account_id(),
        nonce,
        kind: TxKind::Transfer { to, amount },
        public_key: kp.vk.as_bytes().to_vec(),
        sig: vec![],
        relay_node: None,
    };

    tx.sig = kp.sign(&tx.signing_bytes());
    tx
}

#[test]
fn genesis_has_exactly_one_token_total_supply() -> Result<()> {
    let db = sled::Config::new().temporary(true).open()?;
    let ledger = Ledger::from_db(db)?;

    let owner_kp = Keypair::random();
    let owner = owner_kp.account_id();

    ledger.init_genesis("tally-free-test", owner.clone())?;

    let owner_balance = ledger.balance_of(&owner)?;
    assert_eq!(owner_balance.0, *TOTAL_SUPPLY);

    let snapshot = ledger.snapshot()?;
    assert_eq!(sum_balances(&snapshot), *TOTAL_SUPPLY);

    Ok(())
}

#[test]
fn signed_transfer_moves_units_without_changing_supply() -> Result<()> {
    let db = sled::Config::new().temporary(true).open()?;
    let ledger = Ledger::from_db(db)?;

    let owner_kp = Keypair::random();
    let owner = owner_kp.account_id();
    let receiver = AccountId([9u8; 32]);

    ledger.init_genesis("tally-free-test", owner.clone())?;

    let amount = Amount::from_decimal_str("0.25")?;
    let tx = signed_transfer(&owner_kp, receiver.clone(), amount, 0);

    let block = ledger.build_unsigned_block(owner.clone(), 1, vec![tx], BlockCapacity::default())?;

    ledger.commit_block(&block)?;

    let owner_balance = ledger.balance_of(&owner)?;
    let receiver_balance = ledger.balance_of(&receiver)?;

    assert_eq!(owner_balance.to_decimal_string(), "0.75");
    assert_eq!(receiver_balance.to_decimal_string(), "0.25");
    assert_eq!(ledger.nonce_of(&owner)?, 1);

    let snapshot = ledger.snapshot()?;
    assert_eq!(sum_balances(&snapshot), *TOTAL_SUPPLY);

    Ok(())
}

#[test]
fn overspend_is_rejected() -> Result<()> {
    let db = sled::Config::new().temporary(true).open()?;
    let ledger = Ledger::from_db(db)?;

    let owner_kp = Keypair::random();
    let owner = owner_kp.account_id();
    let receiver = AccountId([9u8; 32]);

    ledger.init_genesis("tally-free-test", owner.clone())?;

    let too_much =
        Amount::from_decimal_str("1.000000000000000000000000000000000000000000001")?;

    let tx = signed_transfer(&owner_kp, receiver, too_much, 0);

    let result = ledger.build_unsigned_block(owner, 1, vec![tx], BlockCapacity::default());

    assert!(result.is_err());

    Ok(())
}

#[test]
fn bad_signature_is_rejected() -> Result<()> {
    let db = sled::Config::new().temporary(true).open()?;
    let ledger = Ledger::from_db(db)?;

    let owner_kp = Keypair::random();
    let attacker_kp = Keypair::random();

    let owner = owner_kp.account_id();
    let receiver = AccountId([9u8; 32]);

    ledger.init_genesis("tally-free-test", owner.clone())?;

    let amount = Amount::from_decimal_str("0.25")?;

    let mut tx = SignedTx {
        from: owner.clone(),
        nonce: 0,
        kind: TxKind::Transfer {
            to: receiver,
            amount,
        },
        public_key: owner_kp.vk.as_bytes().to_vec(),
        sig: vec![],
        relay_node: None,
    };

    tx.sig = attacker_kp.sign(&tx.signing_bytes());

    let result = ledger.build_unsigned_block(owner, 1, vec![tx], BlockCapacity::default());

    assert!(result.is_err());

    Ok(())
}

#[test]
fn replay_nonce_is_rejected() -> Result<()> {
    let db = sled::Config::new().temporary(true).open()?;
    let ledger = Ledger::from_db(db)?;

    let owner_kp = Keypair::random();
    let owner = owner_kp.account_id();

    let receiver_a = AccountId([9u8; 32]);
    let receiver_b = AccountId([8u8; 32]);

    ledger.init_genesis("tally-free-test", owner.clone())?;

    let amount_a = Amount::from_decimal_str("0.10")?;
    let tx_a = signed_transfer(&owner_kp, receiver_a, amount_a, 0);

    let block_a =
        ledger.build_unsigned_block(owner.clone(), 1, vec![tx_a], BlockCapacity::default())?;

    ledger.commit_block(&block_a)?;

    let amount_b = Amount::from_decimal_str("0.10")?;

    // Wrong: nonce 0 was already used.
    let tx_b = signed_transfer(&owner_kp, receiver_b, amount_b, 0);

    let result = ledger.build_unsigned_block(owner, 2, vec![tx_b], BlockCapacity::default());

    assert!(result.is_err());

    Ok(())
}