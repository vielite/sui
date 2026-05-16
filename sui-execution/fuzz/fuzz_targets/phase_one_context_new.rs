// Copyright (c) Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

#![no_main]

use libfuzzer_sys::fuzz_target;
use move_vm_runtime::runtime::MoveRuntime;
use prometheus::Registry;
use std::{
    cell::RefCell,
    rc::Rc,
    str::FromStr,
    sync::{Arc, LazyLock},
};
use sui_adapter_latest::{
    adapter::new_move_runtime,
    data_store::{
        cached_package_store::CachedPackageStore,
        transaction_package_store::TransactionPackageStore,
    },
    execution_mode,
    gas_charger::{GasCharger, GasPayment, PaymentKind, PaymentLocation, PaymentMethod},
    static_programmable_transactions::{
        env::Env, execution::context::Context, linkage::analysis::LinkageAnalyzer, loading,
        metering::translation_meter, typing,
    },
    temporary_store::TemporaryStore,
};
use sui_framework::BuiltInFramework;
use sui_move_natives_latest::all_natives;
use sui_protocol_config::{Chain, ProtocolConfig, ProtocolVersion};
use sui_types::{
    TypeTag,
    base_types::{ObjectID, ObjectRef, SuiAddress, TxContext},
    digests::TransactionDigest,
    execution::ExecutionResults,
    gas::SuiGasStatus,
    in_memory_storage::InMemoryStorage,
    metrics::LimitsMetrics,
    object::Object,
    transaction::{
        CallArg, CheckedInputObjects, FundsWithdrawalArg, InputObjects, ObjectArg,
        ObjectReadResult, ProgrammableTransaction,
    },
};

const EPOCH_ID: u64 = 0;
const EPOCH_TIMESTAMP_MS: u64 = 0;
const GAS_PRICE: u64 = 1;
const RGP: u64 = 1;
const MAX_FUZZ_BUDGET: u64 = 1_000_000;
const MAX_EXTRA_BALANCE: u64 = 1_000_000;

static METRICS: LazyLock<Arc<LimitsMetrics>> =
    LazyLock::new(|| Arc::new(LimitsMetrics::new(&Registry::new())));

static STANDARD_CONFIG: LazyLock<ProtocolConfig> =
    LazyLock::new(ProtocolConfig::get_for_max_version_UNSAFE);

static LEGACY_ADDRESS_BALANCE_CONFIG: LazyLock<ProtocolConfig> = LazyLock::new(|| {
    let mut config = ProtocolConfig::get_for_version(ProtocolVersion::new(115), Chain::Unknown);
    config.enable_address_balance_gas_payments_for_testing();
    config
});

static SAFE_ADDRESS_BALANCE_CONFIG: LazyLock<ProtocolConfig> = LazyLock::new(|| {
    let mut config = ProtocolConfig::get_for_max_version_UNSAFE();
    config.enable_address_balance_gas_payments_for_testing();
    config
});

static STANDARD_RUNTIME: LazyLock<Arc<MoveRuntime>> =
    LazyLock::new(|| Arc::new(new_runtime(&STANDARD_CONFIG)));

static LEGACY_ADDRESS_BALANCE_RUNTIME: LazyLock<Arc<MoveRuntime>> =
    LazyLock::new(|| Arc::new(new_runtime(&LEGACY_ADDRESS_BALANCE_CONFIG)));

static SAFE_ADDRESS_BALANCE_RUNTIME: LazyLock<Arc<MoveRuntime>> =
    LazyLock::new(|| Arc::new(new_runtime(&SAFE_ADDRESS_BALANCE_CONFIG)));

static SUI_TYPE_TAG: LazyLock<TypeTag> =
    LazyLock::new(|| TypeTag::from_str("0x2::sui::SUI").expect("valid SUI type tag"));

#[derive(Clone, Copy)]
struct Cursor<'a> {
    data: &'a [u8],
    offset: usize,
}

impl<'a> Cursor<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, offset: 0 }
    }

    fn next_u8(&mut self) -> u8 {
        let Some(byte) = self.data.get(self.offset) else {
            return 0;
        };
        self.offset += 1;
        *byte
    }

    fn next_u64(&mut self) -> u64 {
        let mut bytes = [0u8; 8];
        for byte in &mut bytes {
            *byte = self.next_u8();
        }
        u64::from_le_bytes(bytes)
    }

    fn next_array<const N: usize>(&mut self) -> [u8; N] {
        let mut bytes = [0u8; N];
        for byte in &mut bytes {
            *byte = self.next_u8();
        }
        bytes
    }
}

#[derive(Clone, Copy)]
struct HarnessInput {
    tx_digest: TransactionDigest,
    sender: SuiAddress,
    budget: u64,
    synthetic_amount: u64,
    primary_coin_balance: u64,
    receiving_coin_balance: u64,
    pure_value: u64,
    withdrawal_amount: u64,
}

enum PaymentMode {
    Unmetered,
    AddressBalance { amount: u64 },
    Coin { gas_coin_ref: ObjectRef },
}

#[derive(Clone, Copy)]
enum ExpectedResultKind {
    Empty,
    General,
}

fn new_runtime(config: &ProtocolConfig) -> MoveRuntime {
    new_move_runtime(all_natives(/* silent */ true, config), config)
        .expect("latest Move runtime should build for fuzzing")
}

fn parse_input(data: &[u8]) -> HarnessInput {
    let mut cursor = Cursor::new(data);
    let tx_digest = TransactionDigest::new(cursor.next_array());
    let sender = SuiAddress::from(ObjectID::new(cursor.next_array()));
    let budget = bounded_budget(cursor.next_u64());
    let synthetic_amount = budget + bounded_extra(cursor.next_u64());
    let primary_coin_balance = budget + bounded_extra(cursor.next_u64());
    let receiving_coin_balance = budget + bounded_extra(cursor.next_u64());
    let pure_value = cursor.next_u64() % MAX_EXTRA_BALANCE;
    let withdrawal_amount = 1 + (cursor.next_u64() % budget);

    HarnessInput {
        tx_digest,
        sender,
        budget,
        synthetic_amount,
        primary_coin_balance,
        receiving_coin_balance,
        pure_value,
        withdrawal_amount,
    }
}

fn bounded_budget(raw: u64) -> u64 {
    1 + (raw % MAX_FUZZ_BUDGET)
}

fn bounded_extra(raw: u64) -> u64 {
    1 + (raw % MAX_EXTRA_BALANCE)
}

fn framework_store(extra_objects: Vec<Object>) -> InMemoryStorage {
    let mut objects: Vec<_> = BuiltInFramework::genesis_objects().collect();
    objects.extend(extra_objects);
    InMemoryStorage::new(objects)
}

fn verify_results(kind: ExpectedResultKind, results: ExecutionResults) {
    let ExecutionResults::V2(results) = results else {
        panic!("latest adapter should return v2 execution results");
    };

    assert!(
        results
            .created_object_ids
            .is_disjoint(&results.deleted_object_ids),
        "created and deleted object IDs must stay disjoint",
    );
    assert!(
        results
            .deleted_object_ids
            .iter()
            .all(|id| !results.written_objects.contains_key(id)),
        "deleted objects must not also remain in written objects",
    );

    if matches!(kind, ExpectedResultKind::Empty) {
        assert!(results.written_objects.is_empty(), "expected no writes");
        assert!(
            results.modified_objects.is_empty(),
            "expected no modifications"
        );
        assert!(
            results.created_object_ids.is_empty(),
            "expected no created objects",
        );
        assert!(
            results.deleted_object_ids.is_empty(),
            "expected no deleted objects",
        );
        assert!(results.user_events.is_empty(), "expected no user events");
        assert!(
            results.accumulator_events.is_empty(),
            "expected no accumulator events",
        );
    }
}

fn run_context_case(
    config: &'static ProtocolConfig,
    runtime: &MoveRuntime,
    input: HarnessInput,
    payment_mode: PaymentMode,
    txn: ProgrammableTransaction,
    input_objects: Vec<Object>,
    receiving_objects: Vec<Object>,
    expected_result_kind: ExpectedResultKind,
) {
    let mut store_objects = input_objects.clone();
    store_objects.extend(receiving_objects.clone());
    let backing_store = framework_store(store_objects);
    let input_results = input_objects
        .iter()
        .map(ObjectReadResult::new_from_gas_object)
        .collect::<Vec<_>>();
    let receiving_refs = receiving_objects
        .iter()
        .map(|object| object.compute_object_reference())
        .collect::<Vec<_>>();
    let checked_inputs = CheckedInputObjects::new_for_replay(InputObjects::new(input_results));
    let mut temporary_store = TemporaryStore::new(
        &backing_store,
        checked_inputs.into_inner(),
        receiving_refs,
        input.tx_digest,
        config,
        EPOCH_ID,
    );

    let (mut gas_charger, gas_payment) = match payment_mode {
        PaymentMode::Unmetered => (GasCharger::new_unmetered(input.tx_digest), None),
        PaymentMode::AddressBalance { amount } => {
            let payment_kind = PaymentKind::smash(vec![PaymentMethod::AddressBalance(
                input.sender,
                input.budget,
            )])
            .expect("single address-balance payment must be valid");
            let gas_status =
                SuiGasStatus::new(input.budget, GAS_PRICE, RGP, config).expect("gas status");
            let gas_charger = GasCharger::new(
                input.tx_digest,
                payment_kind,
                gas_status,
                &mut temporary_store,
                config,
            );
            (
                gas_charger,
                Some(GasPayment {
                    location: PaymentLocation::AddressBalance(input.sender),
                    amount,
                }),
            )
        }
        PaymentMode::Coin { gas_coin_ref } => {
            let payment_kind = PaymentKind::smash(vec![PaymentMethod::Coin(gas_coin_ref)])
                .expect("single gas coin payment should be valid");
            let gas_status =
                SuiGasStatus::new(input.budget, GAS_PRICE, RGP, config).expect("gas status");
            let gas_charger = GasCharger::new(
                input.tx_digest,
                payment_kind,
                gas_status,
                &mut temporary_store,
                config,
            );
            let gas_payment = gas_charger.gas_payment_amount();
            (gas_charger, gas_payment)
        }
    };

    let linkage_analysis =
        LinkageAnalyzer::new::<execution_mode::Normal>(config).expect("linkage analysis");
    let package_store =
        CachedPackageStore::new(runtime, TransactionPackageStore::new(&backing_store));

    let Ok(ptb_type_linkage) = linkage_analysis.compute_input_type_resolution_linkage(
        &txn,
        &package_store,
        &temporary_store,
    ) else {
        return;
    };
    let Ok(linkage_context) = ptb_type_linkage.linkage_context() else {
        return;
    };
    let Ok(resolution_vm) = runtime.make_vm(&package_store.package_store, linkage_context) else {
        return;
    };

    let env = Env::new(
        config,
        runtime,
        &mut temporary_store,
        &package_store,
        &linkage_analysis,
        &resolution_vm,
    );
    let tx_context = Rc::new(RefCell::new(TxContext::new_from_components(
        &input.sender,
        &input.tx_digest,
        &EPOCH_ID,
        EPOCH_TIMESTAMP_MS,
        RGP,
        GAS_PRICE,
        input.budget,
        None,
        config,
    )));
    let mut translation_meter = translation_meter::TranslationMeter::new(config, &mut gas_charger);

    let loaded_tx = {
        let tx_context_ref = tx_context.borrow();
        let Ok(loaded_tx) = loading::translate::transaction::<execution_mode::Normal>(
            &mut translation_meter,
            &env,
            &tx_context_ref,
            None,
            gas_payment,
            txn,
        ) else {
            return;
        };
        loaded_tx
    };
    let Ok(typed_tx) = typing::translate_and_verify::<execution_mode::Normal>(
        &mut translation_meter,
        &env,
        loaded_tx,
    ) else {
        return;
    };

    let context = Context::new(
        &env,
        METRICS.clone(),
        tx_context,
        &mut gas_charger,
        typed_tx.gas_payment,
        typed_tx.bytes,
        typed_tx.objects,
        typed_tx.withdrawals,
        typed_tx.pure,
        typed_tx.receiving,
    );
    let Ok(context) = context else {
        return;
    };
    let Ok(results) = context.finish::<execution_mode::Normal>() else {
        return;
    };
    verify_results(expected_result_kind, results);
}

fn empty_txn() -> ProgrammableTransaction {
    ProgrammableTransaction {
        inputs: vec![],
        commands: vec![],
    }
}

fn pure_input_txn(input: HarnessInput) -> ProgrammableTransaction {
    ProgrammableTransaction {
        inputs: vec![CallArg::Pure(input.pure_value.to_le_bytes().to_vec())],
        commands: vec![],
    }
}

fn duplicate_object_inputs_txn(object_ref: ObjectRef) -> ProgrammableTransaction {
    ProgrammableTransaction {
        inputs: vec![
            CallArg::Object(ObjectArg::ImmOrOwnedObject(object_ref)),
            CallArg::Object(ObjectArg::ImmOrOwnedObject(object_ref)),
        ],
        commands: vec![],
    }
}

fn coin_alias_txn(object_ref: ObjectRef) -> ProgrammableTransaction {
    ProgrammableTransaction {
        inputs: vec![CallArg::Object(ObjectArg::ImmOrOwnedObject(object_ref))],
        commands: vec![],
    }
}

fn receiving_input_txn(object_ref: ObjectRef) -> ProgrammableTransaction {
    ProgrammableTransaction {
        inputs: vec![CallArg::Object(ObjectArg::Receiving(object_ref))],
        commands: vec![],
    }
}

fn withdrawal_input_txn(input: HarnessInput) -> ProgrammableTransaction {
    ProgrammableTransaction {
        inputs: vec![CallArg::FundsWithdrawal(
            FundsWithdrawalArg::balance_from_sender(input.withdrawal_amount, SUI_TYPE_TAG.clone()),
        )],
        commands: vec![],
    }
}

fn run_all_cases(data: &[u8]) {
    let input = parse_input(data);
    let primary = Object::new_gas_with_balance_and_owner_for_testing(
        input.primary_coin_balance,
        input.sender,
    );
    let receiving = Object::new_gas_with_balance_and_owner_for_testing(
        input.receiving_coin_balance,
        input.sender,
    );

    run_context_case(
        &STANDARD_CONFIG,
        STANDARD_RUNTIME.as_ref(),
        input,
        PaymentMode::Unmetered,
        empty_txn(),
        vec![],
        vec![],
        ExpectedResultKind::Empty,
    );
    run_context_case(
        &STANDARD_CONFIG,
        STANDARD_RUNTIME.as_ref(),
        input,
        PaymentMode::Unmetered,
        pure_input_txn(input),
        vec![],
        vec![],
        ExpectedResultKind::Empty,
    );
    run_context_case(
        &STANDARD_CONFIG,
        STANDARD_RUNTIME.as_ref(),
        input,
        PaymentMode::Unmetered,
        duplicate_object_inputs_txn(primary.compute_object_reference()),
        vec![primary.clone()],
        vec![],
        ExpectedResultKind::General,
    );
    run_context_case(
        &LEGACY_ADDRESS_BALANCE_CONFIG,
        LEGACY_ADDRESS_BALANCE_RUNTIME.as_ref(),
        input,
        PaymentMode::AddressBalance {
            amount: input.synthetic_amount,
        },
        empty_txn(),
        vec![],
        vec![],
        ExpectedResultKind::Empty,
    );
    run_context_case(
        &SAFE_ADDRESS_BALANCE_CONFIG,
        SAFE_ADDRESS_BALANCE_RUNTIME.as_ref(),
        input,
        PaymentMode::AddressBalance {
            amount: input.synthetic_amount,
        },
        empty_txn(),
        vec![],
        vec![],
        ExpectedResultKind::Empty,
    );
    run_context_case(
        &STANDARD_CONFIG,
        STANDARD_RUNTIME.as_ref(),
        input,
        PaymentMode::Coin {
            gas_coin_ref: primary.compute_object_reference(),
        },
        coin_alias_txn(primary.compute_object_reference()),
        vec![primary.clone()],
        vec![],
        ExpectedResultKind::General,
    );

    if STANDARD_CONFIG.receiving_objects_supported() {
        run_context_case(
            &STANDARD_CONFIG,
            STANDARD_RUNTIME.as_ref(),
            input,
            PaymentMode::Unmetered,
            receiving_input_txn(receiving.compute_object_reference()),
            vec![],
            vec![receiving.clone()],
            ExpectedResultKind::Empty,
        );
    }

    if STANDARD_CONFIG.enable_accumulators() {
        run_context_case(
            &STANDARD_CONFIG,
            STANDARD_RUNTIME.as_ref(),
            input,
            PaymentMode::Unmetered,
            withdrawal_input_txn(input),
            vec![],
            vec![],
            ExpectedResultKind::Empty,
        );
    }
}

fuzz_target!(|data: &[u8]| {
    run_all_cases(data);
});
