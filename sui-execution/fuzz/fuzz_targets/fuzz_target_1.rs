// Copyright (c) Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

#![no_main]

use indexmap::IndexSet;
use libfuzzer_sys::fuzz_target;
use move_vm_runtime::runtime::MoveRuntime;
use prometheus::Registry;
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, LazyLock},
};
use sui_adapter_latest::{
    adapter::new_move_runtime,
    data_store::{
        cached_package_store::CachedPackageStore,
        transaction_package_store::TransactionPackageStore,
    },
    execution_engine::execute_transaction_to_effects,
    execution_mode,
    gas_charger::{GasCharger, GasPayment, PaymentKind, PaymentLocation, PaymentMethod},
    static_programmable_transactions::{
        env::Env, execution::context::Context, linkage::analysis::LinkageAnalyzer,
    },
    temporary_store::TemporaryStore,
};
use sui_framework::BuiltInFramework;
use sui_move_natives_latest::all_natives;
use sui_protocol_config::{Chain, ProtocolConfig, ProtocolVersion};
use sui_types::{
    base_types::{ObjectID, SuiAddress, TxContext},
    digests::TransactionDigest,
    execution_params::ExecutionOrEarlyError,
    gas::SuiGasStatus,
    in_memory_storage::InMemoryStorage,
    metrics::LimitsMetrics,
    object::Object,
    programmable_transaction_builder::ProgrammableTransactionBuilder,
    transaction::{
        CheckedInputObjects, GasData, InputObjects, ObjectReadResult, ProgrammableTransaction,
        TransactionKind,
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
    LazyLock::new(|| new_runtime(&STANDARD_CONFIG));

static LEGACY_ADDRESS_BALANCE_RUNTIME: LazyLock<Arc<MoveRuntime>> =
    LazyLock::new(|| new_runtime(&LEGACY_ADDRESS_BALANCE_CONFIG));

static SAFE_ADDRESS_BALANCE_RUNTIME: LazyLock<Arc<MoveRuntime>> =
    LazyLock::new(|| new_runtime(&SAFE_ADDRESS_BALANCE_CONFIG));

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
    recipient: SuiAddress,
    budget: u64,
    synthetic_amount: u64,
    primary_coin_balance: u64,
    secondary_coin_balance: u64,
}

enum DirectPayment {
    Unmetered,
    AddressBalance { amount: u64 },
    Coin { balances: [u64; 2] },
}

fn new_runtime(config: &ProtocolConfig) -> Arc<MoveRuntime> {
    Arc::new(
        new_move_runtime(all_natives(/* silent */ true, config), config)
            .expect("latest Move runtime should build for fuzzing"),
    )
}

fn parse_input(data: &[u8]) -> HarnessInput {
    let mut cursor = Cursor::new(data);
    let tx_digest = TransactionDigest::new(cursor.next_array());
    let sender = SuiAddress::from(ObjectID::new(cursor.next_array()));
    let recipient = SuiAddress::from(ObjectID::new(cursor.next_array()));
    let budget = bounded_budget(cursor.next_u64());
    let synthetic_amount = budget + bounded_extra(cursor.next_u64());
    let primary_coin_balance = budget + bounded_extra(cursor.next_u64());
    let secondary_coin_balance = 1 + (cursor.next_u64() % MAX_EXTRA_BALANCE);

    HarnessInput {
        tx_digest,
        sender,
        recipient,
        budget,
        synthetic_amount,
        primary_coin_balance,
        secondary_coin_balance,
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

fn run_context_new_case(
    config: &'static ProtocolConfig,
    runtime: &MoveRuntime,
    input: HarnessInput,
    payment: DirectPayment,
) {
    let (extra_objects, input_results) = match payment {
        DirectPayment::Coin { balances } => {
            let primary =
                Object::new_gas_with_balance_and_owner_for_testing(balances[0], input.sender);
            let secondary =
                Object::new_gas_with_balance_and_owner_for_testing(balances[1], input.sender);
            let inputs = vec![
                ObjectReadResult::new_from_gas_object(&primary),
                ObjectReadResult::new_from_gas_object(&secondary),
            ];
            (vec![primary, secondary], inputs)
        }
        DirectPayment::Unmetered | DirectPayment::AddressBalance { .. } => (vec![], vec![]),
    };

    let backing_store = framework_store(extra_objects.clone());
    let checked_inputs = CheckedInputObjects::new_for_replay(InputObjects::new(input_results));
    let mut temporary_store = TemporaryStore::new(
        &backing_store,
        checked_inputs.into_inner(),
        vec![],
        input.tx_digest,
        config,
        EPOCH_ID,
    );

    let (mut gas_charger, payment_location) = match payment {
        DirectPayment::Unmetered => (GasCharger::new_unmetered(input.tx_digest), None),
        DirectPayment::AddressBalance { amount } => {
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
        DirectPayment::Coin { balances: _ } => {
            let payment_methods = extra_objects
                .iter()
                .map(|object| PaymentMethod::Coin(object.compute_object_reference()))
                .collect();
            let payment_kind =
                PaymentKind::smash(payment_methods).expect("coin payments should be unique");
            let gas_status =
                SuiGasStatus::new(input.budget, GAS_PRICE, RGP, config).expect("gas status");
            let gas_charger = GasCharger::new(
                input.tx_digest,
                payment_kind,
                gas_status,
                &mut temporary_store,
                config,
            );
            let payment_location = gas_charger.gas_payment_amount();
            (gas_charger, payment_location)
        }
    };

    let linkage_analysis =
        LinkageAnalyzer::new::<execution_mode::Normal>(config).expect("linkage analysis");
    let package_store =
        CachedPackageStore::new(runtime, TransactionPackageStore::new(&backing_store));
    let empty_pt = ProgrammableTransaction {
        inputs: vec![],
        commands: vec![],
    };
    let linkage_context = linkage_analysis
        .compute_input_type_resolution_linkage(&empty_pt, &package_store, &temporary_store)
        .and_then(|linkage| linkage.linkage_context())
        .expect("empty PT should resolve linkage");
    let resolution_vm = runtime
        .make_vm(&package_store.package_store, linkage_context)
        .expect("resolution VM for empty PT");

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

    let context = Context::new(
        &env,
        METRICS.clone(),
        tx_context,
        &mut gas_charger,
        payment_location,
        IndexSet::new(),
        vec![],
        vec![],
        vec![],
        vec![],
    );

    if let Ok(context) = context {
        let _ = context.finish::<execution_mode::Normal>();
    }
}

fn run_execute_move_case(input: HarnessInput) {
    let config = &SAFE_ADDRESS_BALANCE_CONFIG;
    let runtime = &SAFE_ADDRESS_BALANCE_RUNTIME;
    let backing_store = framework_store(vec![]);
    let checked_inputs = CheckedInputObjects::new_for_replay(InputObjects::new(vec![]));

    let mut builder = ProgrammableTransactionBuilder::new();
    builder.pay_all_sui(input.recipient);
    let tx_kind = TransactionKind::ProgrammableTransaction(builder.finish());
    let gas_data = GasData {
        payment: vec![],
        owner: input.sender,
        price: GAS_PRICE,
        budget: input.budget,
    };
    let gas_status = SuiGasStatus::new(input.budget, GAS_PRICE, RGP, config).expect("gas status");
    let execution_params: ExecutionOrEarlyError = Ok(());
    let mut trace_builder = None;

    let _ = execute_transaction_to_effects::<execution_mode::Normal>(
        &backing_store,
        checked_inputs,
        gas_data,
        gas_status,
        tx_kind,
        None,
        input.sender,
        input.tx_digest,
        runtime,
        &EPOCH_ID,
        EPOCH_TIMESTAMP_MS,
        config,
        METRICS.clone(),
        false,
        execution_params,
        &mut trace_builder,
    );
}

fuzz_target!(|data: &[u8]| {
    let input = parse_input(data);

    run_context_new_case(
        &STANDARD_CONFIG,
        STANDARD_RUNTIME.as_ref(),
        input,
        DirectPayment::Unmetered,
    );
    run_context_new_case(
        &LEGACY_ADDRESS_BALANCE_CONFIG,
        LEGACY_ADDRESS_BALANCE_RUNTIME.as_ref(),
        input,
        DirectPayment::AddressBalance {
            amount: input.synthetic_amount,
        },
    );
    run_context_new_case(
        &SAFE_ADDRESS_BALANCE_CONFIG,
        SAFE_ADDRESS_BALANCE_RUNTIME.as_ref(),
        input,
        DirectPayment::AddressBalance {
            amount: input.synthetic_amount,
        },
    );
    run_context_new_case(
        &STANDARD_CONFIG,
        STANDARD_RUNTIME.as_ref(),
        input,
        DirectPayment::Coin {
            balances: [input.primary_coin_balance, input.secondary_coin_balance],
        },
    );

    run_execute_move_case(input);
});
