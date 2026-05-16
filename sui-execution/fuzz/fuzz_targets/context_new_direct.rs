// Copyright (c) Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

#![no_main]

use indexmap::IndexSet;
use libfuzzer_sys::fuzz_target;
use move_vm_runtime::{execution::values::Reference, runtime::MoveRuntime};
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
        env::Env, execution::context::Context, linkage::analysis::LinkageAnalyzer,
        loading::ast as L, spanned::sp, typing::ast as T,
    },
    temporary_store::TemporaryStore,
};
use sui_framework::BuiltInFramework;
use sui_move_natives_latest::all_natives;
use sui_protocol_config::{Chain, ProtocolConfig, ProtocolVersion};
use sui_types::{
    base_types::{ObjectID, ObjectRef, SuiAddress, TxContext},
    digests::TransactionDigest,
    execution::ExecutionResults,
    funds_accumulator::Withdrawal,
    gas::SuiGasStatus,
    in_memory_storage::InMemoryStorage,
    metrics::LimitsMetrics,
    object::Object,
    transaction::{
        CallArg, CheckedInputObjects, FundsWithdrawalArg, InputObjects,
        ObjectArg as TransactionObjectArg, ObjectReadResult, ProgrammableTransaction,
    },
    TypeTag,
};

const EPOCH_ID: u64 = 0;
const EPOCH_TIMESTAMP_MS: u64 = 0;
const GAS_PRICE: u64 = 1;
const RGP: u64 = 1;
const MAX_FUZZ_BUDGET: u64 = 1_000_000;
const MAX_EXTRA_BALANCE: u64 = 1_000_000;
const MAX_OBJECT_INPUTS: usize = 4;
const MAX_PURE_INPUTS: usize = 4;
const MAX_RECEIVING_INPUTS: usize = 2;
const MAX_WITHDRAWAL_INPUTS: usize = 2;

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

    fn next_bool(&mut self) -> bool {
        self.next_u8() & 1 == 1
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

    fn next_usize(&mut self, max: usize) -> usize {
        if max == 0 {
            0
        } else {
            usize::from(self.next_u8()) % max
        }
    }
}

#[derive(Clone, Copy)]
struct HarnessInput {
    tx_digest: TransactionDigest,
    sender: SuiAddress,
    budget: u64,
    synthetic_amount: u64,
    primary_coin_balance: u64,
    secondary_coin_balance: u64,
    receiving_coin_balance: u64,
    pure_value: u64,
    alternate_pure_value: u64,
    withdrawal_amount: u64,
}

enum PaymentMode {
    Unmetered,
    AddressBalance { amount: u64 },
    Coin,
}

fn new_runtime(config: &ProtocolConfig) -> MoveRuntime {
    new_move_runtime(all_natives(/* silent */ true, config), config)
        .expect("latest Move runtime should build for fuzzing")
}

fn parse_input(data: &[u8]) -> (HarnessInput, Cursor<'_>) {
    let mut cursor = Cursor::new(data);
    let tx_digest = TransactionDigest::new(cursor.next_array());
    let sender = SuiAddress::from(ObjectID::new(cursor.next_array()));
    let budget = bounded_budget(cursor.next_u64());
    let synthetic_amount = budget + bounded_extra(cursor.next_u64());
    let primary_coin_balance = budget + bounded_extra(cursor.next_u64());
    let secondary_coin_balance = budget + bounded_extra(cursor.next_u64());
    let receiving_coin_balance = budget + bounded_extra(cursor.next_u64());
    let pure_value = cursor.next_u64() % MAX_EXTRA_BALANCE;
    let alternate_pure_value = cursor.next_u64() % MAX_EXTRA_BALANCE;
    let withdrawal_amount = 1 + (cursor.next_u64() % budget);

    (
        HarnessInput {
            tx_digest,
            sender,
            budget,
            synthetic_amount,
            primary_coin_balance,
            secondary_coin_balance,
            receiving_coin_balance,
            pure_value,
            alternate_pure_value,
            withdrawal_amount,
        },
        cursor,
    )
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

fn verify_results(results: ExecutionResults) {
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
}

fn linkage_seed_txn(
    config: &ProtocolConfig,
    input: HarnessInput,
    primary_ref: ObjectRef,
    receiving_ref: ObjectRef,
) -> ProgrammableTransaction {
    let mut inputs = vec![
        CallArg::Object(TransactionObjectArg::ImmOrOwnedObject(primary_ref)),
        CallArg::Pure(input.pure_value.to_le_bytes().to_vec()),
    ];

    if config.receiving_objects_supported() {
        inputs.push(CallArg::Object(TransactionObjectArg::Receiving(
            receiving_ref,
        )));
    }

    if config.enable_accumulators() {
        inputs.push(CallArg::FundsWithdrawal(
            FundsWithdrawalArg::balance_from_sender(input.withdrawal_amount, SUI_TYPE_TAG.clone()),
        ));
    }

    ProgrammableTransaction {
        inputs,
        commands: vec![],
    }
}

fn build_object_inputs(
    cursor: &mut Cursor<'_>,
    ty: &T::Type,
    primary_ref: ObjectRef,
    secondary_ref: ObjectRef,
) -> Vec<T::ObjectInput> {
    (0..cursor.next_usize(MAX_OBJECT_INPUTS + 1))
        .map(|_| {
            let object_ref = if cursor.next_bool() {
                primary_ref
            } else {
                secondary_ref
            };
            T::ObjectInput {
                original_input_index: T::InputIndex(u16::from(cursor.next_u8())),
                arg: L::ObjectArg::OwnedObject(object_ref),
                ty: ty.clone(),
            }
        })
        .collect()
}

fn build_pure_inputs(
    cursor: &mut Cursor<'_>,
    primary_value: u64,
    alternate_value: u64,
) -> (IndexSet<Vec<u8>>, Vec<T::PureInput>) {
    let mut bytes = IndexSet::new();
    bytes.insert(primary_value.to_le_bytes().to_vec());
    bytes.insert(alternate_value.to_le_bytes().to_vec());

    let pure_inputs = (0..cursor.next_usize(MAX_PURE_INPUTS + 1))
        .map(|_| T::PureInput {
            original_input_index: T::InputIndex(u16::from(cursor.next_u8())),
            byte_index: cursor.next_usize(bytes.len()),
            ty: T::Type::U64,
            constraint: T::BytesConstraint {
                command: u16::from(cursor.next_u8()),
                argument: u16::from(cursor.next_u8()),
            },
        })
        .collect();

    (bytes, pure_inputs)
}

fn build_receiving_inputs(
    cursor: &mut Cursor<'_>,
    ty: &T::Type,
    receiving_ref: ObjectRef,
    enabled: bool,
) -> Vec<T::ReceivingInput> {
    if !enabled {
        return vec![];
    }

    (0..cursor.next_usize(MAX_RECEIVING_INPUTS + 1))
        .map(|_| T::ReceivingInput {
            original_input_index: T::InputIndex(u16::from(cursor.next_u8())),
            object_ref: receiving_ref,
            ty: ty.clone(),
            constraint: T::BytesConstraint {
                command: u16::from(cursor.next_u8()),
                argument: u16::from(cursor.next_u8()),
            },
        })
        .collect()
}

fn build_withdrawal_inputs(
    cursor: &mut Cursor<'_>,
    input: HarnessInput,
    ty: Option<&T::Type>,
    enabled: bool,
) -> Vec<T::WithdrawalInput> {
    let Some(ty) = ty else {
        return vec![];
    };
    if !enabled {
        return vec![];
    }

    (0..cursor.next_usize(MAX_WITHDRAWAL_INPUTS + 1))
        .map(|_| T::WithdrawalInput {
            original_input_index: T::InputIndex(u16::from(cursor.next_u8())),
            ty: ty.clone(),
            owner: input.sender.into(),
            amount: (1 + (cursor.next_u64() % input.synthetic_amount)).into(),
        })
        .collect()
}

fn arg_borrow(location: T::Location, idx: u16) -> T::Argument {
    sp(idx, (T::Argument__::Borrow(false, location), T::Type::U64))
}

fn arg_copy(location: T::Location, idx: u16) -> T::Argument {
    sp(idx, (T::Argument__::new_copy(location), T::Type::U64))
}

fn borrow_location<'env, 'pc, 'vm, 'state, 'linkage, 'gas, 'extension>(
    context: &mut Context<'env, 'pc, 'vm, 'state, 'linkage, 'gas, 'extension>,
    location: T::Location,
    idx: u16,
) {
    let Ok(reference) = context.argument::<Reference>(arg_borrow(location, idx)) else {
        return;
    };
    drop(reference);
}

fn copy_pure_input<'env, 'pc, 'vm, 'state, 'linkage, 'gas, 'extension>(
    context: &mut Context<'env, 'pc, 'vm, 'state, 'linkage, 'gas, 'extension>,
    idx: u16,
) {
    let Ok(value) = context.argument::<u64>(arg_copy(T::Location::PureInput(idx), idx)) else {
        return;
    };
    let _ = value;
}

fn runtime_has_gas_local(config: &ProtocolConfig, gas_payment: Option<GasPayment>) -> bool {
    let Some(gas_payment) = gas_payment else {
        return false;
    };

    match gas_payment.location {
        PaymentLocation::Coin(_) => true,
        PaymentLocation::AddressBalance(_) => config.gasless_transaction_drop_safety(),
    }
}

fn touch_context<'env, 'pc, 'vm, 'state, 'linkage, 'gas, 'extension>(
    context: &mut Context<'env, 'pc, 'vm, 'state, 'linkage, 'gas, 'extension>,
    mut cursor: Cursor<'_>,
    gas_present: bool,
    object_count: usize,
    pure_count: usize,
    receiving_count: usize,
    withdrawal_count: usize,
) {
    if gas_present {
        borrow_location(context, T::Location::GasCoin, 0);
    }

    if object_count > 0 {
        let idx = cursor.next_usize(object_count) as u16;
        borrow_location(context, T::Location::ObjectInput(idx), idx);
        if cursor.next_bool() {
            let second_idx = cursor.next_usize(object_count) as u16;
            borrow_location(context, T::Location::ObjectInput(second_idx), second_idx);
        }
    }

    if pure_count > 0 {
        let idx = cursor.next_usize(pure_count) as u16;
        copy_pure_input(context, idx);
        if cursor.next_bool() {
            copy_pure_input(context, idx);
        }
    }

    if receiving_count > 0 {
        let idx = cursor.next_usize(receiving_count) as u16;
        borrow_location(context, T::Location::ReceivingInput(idx), idx);
    }

    if withdrawal_count > 0 {
        let idx = cursor.next_usize(withdrawal_count) as u16;
        borrow_location(context, T::Location::WithdrawalInput(idx), idx);
    }
}

fn run_direct_case(
    config: &'static ProtocolConfig,
    runtime: &MoveRuntime,
    input: HarnessInput,
    payment_mode: PaymentMode,
    mut cursor: Cursor<'_>,
) {
    let primary = Object::new_gas_with_balance_and_owner_for_testing(
        input.primary_coin_balance,
        input.sender,
    );
    let secondary = Object::new_gas_with_balance_and_owner_for_testing(
        input.secondary_coin_balance,
        input.sender,
    );
    let receiving = Object::new_gas_with_balance_and_owner_for_testing(
        input.receiving_coin_balance,
        input.sender,
    );
    let primary_ref = primary.compute_object_reference();
    let secondary_ref = secondary.compute_object_reference();
    let receiving_ref = receiving.compute_object_reference();

    let backing_store =
        framework_store(vec![primary.clone(), secondary.clone(), receiving.clone()]);
    let input_results = [primary.clone(), secondary.clone()]
        .into_iter()
        .map(|object| ObjectReadResult::new_from_gas_object(&object))
        .collect::<Vec<_>>();
    let receiving_refs = if config.receiving_objects_supported() {
        vec![receiving_ref]
    } else {
        vec![]
    };
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
        PaymentMode::Coin => {
            let payment_kind = PaymentKind::smash(vec![PaymentMethod::Coin(primary_ref)])
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
    let gas_present = runtime_has_gas_local(config, gas_payment);

    let linkage_analysis =
        LinkageAnalyzer::new::<execution_mode::Normal>(config).expect("linkage analysis");
    let package_store =
        CachedPackageStore::new(runtime, TransactionPackageStore::new(&backing_store));
    let linkage_txn = linkage_seed_txn(config, input, primary_ref, receiving_ref);
    let Ok(ptb_type_linkage) = linkage_analysis.compute_input_type_resolution_linkage(
        &linkage_txn,
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
    let Ok(gas_coin_type) = env.gas_coin_type() else {
        return;
    };
    let withdrawal_type = if config.enable_accumulators() {
        env.load_type_from_struct(&Withdrawal::type_(SUI_TYPE_TAG.clone()))
            .ok()
    } else {
        None
    };
    let (pure_input_bytes, pure_inputs) =
        build_pure_inputs(&mut cursor, input.pure_value, input.alternate_pure_value);
    let object_inputs =
        build_object_inputs(&mut cursor, &gas_coin_type, primary_ref, secondary_ref);
    let receiving_inputs = build_receiving_inputs(
        &mut cursor,
        &gas_coin_type,
        receiving_ref,
        config.receiving_objects_supported(),
    );
    let withdrawal_inputs = build_withdrawal_inputs(
        &mut cursor,
        input,
        withdrawal_type.as_ref(),
        config.enable_accumulators(),
    );
    let object_count = object_inputs.len();
    let pure_count = pure_inputs.len();
    let receiving_count = receiving_inputs.len();
    let withdrawal_count = withdrawal_inputs.len();

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

    let Ok(mut context) = Context::new(
        &env,
        METRICS.clone(),
        tx_context,
        &mut gas_charger,
        gas_payment,
        pure_input_bytes,
        object_inputs,
        withdrawal_inputs,
        pure_inputs,
        receiving_inputs,
    ) else {
        return;
    };

    touch_context(
        &mut context,
        cursor,
        gas_present,
        object_count,
        pure_count,
        receiving_count,
        withdrawal_count,
    );

    let Ok(results) = context.finish::<execution_mode::Normal>() else {
        return;
    };
    verify_results(results);
}

fn run_all_cases(data: &[u8]) {
    let (input, cursor) = parse_input(data);

    let standard_unmetered_cursor = cursor;
    let mut standard_coin_cursor = cursor;
    let _ = standard_coin_cursor.next_u8();
    let mut legacy_address_cursor = cursor;
    let _ = legacy_address_cursor.next_u8();
    let _ = legacy_address_cursor.next_u8();
    let mut safe_address_cursor = cursor;
    let _ = safe_address_cursor.next_u8();
    let _ = safe_address_cursor.next_u8();
    let _ = safe_address_cursor.next_u8();

    run_direct_case(
        &STANDARD_CONFIG,
        STANDARD_RUNTIME.as_ref(),
        input,
        PaymentMode::Unmetered,
        standard_unmetered_cursor,
    );
    run_direct_case(
        &STANDARD_CONFIG,
        STANDARD_RUNTIME.as_ref(),
        input,
        PaymentMode::Coin,
        standard_coin_cursor,
    );
    run_direct_case(
        &LEGACY_ADDRESS_BALANCE_CONFIG,
        LEGACY_ADDRESS_BALANCE_RUNTIME.as_ref(),
        input,
        PaymentMode::AddressBalance {
            amount: input.budget,
        },
        legacy_address_cursor,
    );
    run_direct_case(
        &SAFE_ADDRESS_BALANCE_CONFIG,
        SAFE_ADDRESS_BALANCE_RUNTIME.as_ref(),
        input,
        PaymentMode::AddressBalance {
            amount: input.synthetic_amount,
        },
        safe_address_cursor,
    );
}

fuzz_target!(|data: &[u8]| {
    run_all_cases(data);
});
