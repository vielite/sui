// Copyright (c) Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

#![no_main]

use libfuzzer_sys::fuzz_target;
use move_vm_runtime::runtime::MoveRuntime;
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
    gas_charger::GasCharger,
    static_programmable_transactions::{
        env::Env, linkage::analysis::LinkageAnalyzer, loading, metering::translation_meter, typing,
    },
    temporary_store::TemporaryStore,
};
use sui_framework::BuiltInFramework;
use sui_move_natives_latest::all_natives;
use sui_protocol_config::ProtocolConfig;
use sui_types::{
    BRIDGE_PACKAGE_ID, DEEPBOOK_PACKAGE_ID, MOVE_STDLIB_PACKAGE_ID, SUI_FRAMEWORK_PACKAGE_ID,
    SUI_SYSTEM_PACKAGE_ID, TypeTag,
    base_types::{ObjectID, SuiAddress, TxContext},
    digests::TransactionDigest,
    in_memory_storage::InMemoryStorage,
    object::Object,
    programmable_transaction_builder::ProgrammableTransactionBuilder,
    transaction::{
        Argument, CheckedInputObjects, Command, InputObjects, ObjectArg, ObjectReadResult,
        ProgrammableMoveCall, ProgrammableTransaction,
    },
    type_input::TypeInput,
};

const EPOCH_ID: u64 = 0;
const EPOCH_TIMESTAMP_MS: u64 = 0;
const GAS_PRICE: u64 = 1;
const RGP: u64 = 1;
const MAX_FUZZ_BUDGET: u64 = 1_000_000;
const MAX_EXTRA_BALANCE: u64 = 1_000_000;
const MAX_COMMANDS: usize = 4;
const MAX_DEPENDENCIES: usize = 4;
const MAX_MODULES: usize = 3;
const MAX_MODULE_BYTES: usize = 16;

static STANDARD_CONFIG: LazyLock<ProtocolConfig> =
    LazyLock::new(ProtocolConfig::get_for_max_version_UNSAFE);

static STANDARD_RUNTIME: LazyLock<Arc<MoveRuntime>> =
    LazyLock::new(|| Arc::new(new_runtime(&STANDARD_CONFIG)));

static SUI_TYPE_TAG: LazyLock<TypeTag> =
    LazyLock::new(|| TypeTag::from_str("0x2::sui::SUI").expect("valid SUI type tag"));

static COIN_SUI_TYPE_TAG: LazyLock<TypeTag> = LazyLock::new(|| {
    TypeTag::from_str("0x2::coin::Coin<0x2::sui::SUI>").expect("valid Coin<SUI> type tag")
});

static BALANCE_SUI_TYPE_TAG: LazyLock<TypeTag> = LazyLock::new(|| {
    TypeTag::from_str("0x2::balance::Balance<0x2::sui::SUI>").expect("valid Balance<SUI> type tag")
});

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

    fn next_bytes(&mut self, max_len: usize) -> Vec<u8> {
        let len = self.next_usize(max_len + 1);
        (0..len).map(|_| self.next_u8()).collect()
    }
}

#[derive(Clone, Copy)]
struct HarnessInput {
    tx_digest: TransactionDigest,
    sender: SuiAddress,
    recipient: SuiAddress,
    budget: u64,
    primary_coin_balance: u64,
    secondary_coin_balance: u64,
}

fn new_runtime(config: &ProtocolConfig) -> MoveRuntime {
    new_move_runtime(all_natives(/* silent */ true, config), config)
        .expect("latest Move runtime should build for fuzzing")
}

fn parse_input(cursor: &mut Cursor<'_>) -> HarnessInput {
    let tx_digest = TransactionDigest::new(cursor.next_array());
    let sender = SuiAddress::from(ObjectID::new(cursor.next_array()));
    let recipient = SuiAddress::from(ObjectID::new(cursor.next_array()));
    let budget = bounded_budget(cursor.next_u64());
    let primary_coin_balance = budget + bounded_extra(cursor.next_u64());
    let secondary_coin_balance = budget + bounded_extra(cursor.next_u64());

    HarnessInput {
        tx_digest,
        sender,
        recipient,
        budget,
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

fn framework_store() -> InMemoryStorage {
    InMemoryStorage::new(BuiltInFramework::genesis_objects().collect())
}

fn select_package_id(cursor: &mut Cursor<'_>) -> ObjectID {
    if cursor.next_bool() {
        match cursor.next_u8() % 5 {
            0 => MOVE_STDLIB_PACKAGE_ID,
            1 => SUI_FRAMEWORK_PACKAGE_ID,
            2 => SUI_SYSTEM_PACKAGE_ID,
            3 => BRIDGE_PACKAGE_ID,
            _ => DEEPBOOK_PACKAGE_ID,
        }
    } else {
        ObjectID::new(cursor.next_array())
    }
}

fn select_type_tag(cursor: &mut Cursor<'_>) -> TypeTag {
    match cursor.next_u8() % 6 {
        0 => SUI_TYPE_TAG.clone(),
        1 => COIN_SUI_TYPE_TAG.clone(),
        2 => BALANCE_SUI_TYPE_TAG.clone(),
        3 => TypeTag::U64,
        4 => TypeTag::Vector(Box::new(TypeTag::Address)),
        _ => TypeTag::from_str(&format!("0x{:x}::ghost::Phantom", 1 + cursor.next_u8()))
            .expect("ghost type tag should parse"),
    }
}

fn select_module(cursor: &mut Cursor<'_>) -> String {
    match cursor.next_u8() % 5 {
        0 => "sui",
        1 => "coin",
        2 => "balance",
        3 => "pay",
        _ => "ghost",
    }
    .to_owned()
}

fn select_function(cursor: &mut Cursor<'_>) -> String {
    match cursor.next_u8() % 5 {
        0 => "transfer",
        1 => "value",
        2 => "split",
        3 => "join",
        _ => "missing",
    }
    .to_owned()
}

fn build_module_blobs(cursor: &mut Cursor<'_>) -> Vec<Vec<u8>> {
    (0..cursor.next_usize(MAX_MODULES))
        .map(|_| cursor.next_bytes(MAX_MODULE_BYTES))
        .collect()
}

fn build_dependency_ids(cursor: &mut Cursor<'_>) -> Vec<ObjectID> {
    (0..cursor.next_usize(MAX_DEPENDENCIES))
        .map(|_| select_package_id(cursor))
        .collect()
}

fn choose_input_arg(cursor: &mut Cursor<'_>, input_args: &[Argument]) -> Argument {
    input_args[cursor.next_usize(input_args.len())]
}

fn choose_any_arg(
    cursor: &mut Cursor<'_>,
    input_args: &[Argument],
    result_args: &[Argument],
) -> Argument {
    if !result_args.is_empty() && cursor.next_bool() {
        result_args[cursor.next_usize(result_args.len())]
    } else {
        choose_input_arg(cursor, input_args)
    }
}

fn build_known_transfer_move_call(
    cursor: &mut Cursor<'_>,
    object_args: &[Argument],
    recipient_arg: Argument,
) -> Command {
    let object_arg = object_args[cursor.next_usize(object_args.len())];
    Command::MoveCall(Box::new(ProgrammableMoveCall {
        package: SUI_FRAMEWORK_PACKAGE_ID,
        module: "sui".to_owned(),
        function: "transfer".to_owned(),
        type_arguments: vec![],
        arguments: vec![object_arg, recipient_arg],
    }))
}

fn build_fuzzed_move_call(
    cursor: &mut Cursor<'_>,
    input_args: &[Argument],
    result_args: &[Argument],
) -> Command {
    let type_arguments = (0..cursor.next_usize(3))
        .map(|_| TypeInput::from(select_type_tag(cursor)))
        .collect();
    let arguments = (0..cursor.next_usize(4))
        .map(|_| choose_any_arg(cursor, input_args, result_args))
        .collect();
    Command::MoveCall(Box::new(ProgrammableMoveCall {
        package: select_package_id(cursor),
        module: select_module(cursor),
        function: select_function(cursor),
        type_arguments,
        arguments,
    }))
}

fn build_make_move_vec(
    cursor: &mut Cursor<'_>,
    input_args: &[Argument],
    result_args: &[Argument],
) -> Command {
    let ty = cursor
        .next_bool()
        .then(|| TypeInput::from(select_type_tag(cursor)));
    let arguments = (0..cursor.next_usize(4))
        .map(|_| choose_any_arg(cursor, input_args, result_args))
        .collect();
    Command::MakeMoveVec(ty, arguments)
}

fn build_transaction(
    cursor: &mut Cursor<'_>,
    input: HarnessInput,
    coin_refs: &[(
        ObjectID,
        sui_types::base_types::SequenceNumber,
        sui_types::digests::ObjectDigest,
    )],
) -> ProgrammableTransaction {
    let mut builder = ProgrammableTransactionBuilder::new();
    let recipient_arg = builder.pure(input.recipient).expect("recipient address");
    let sender_arg = builder.pure(input.sender).expect("sender address");
    let amount_arg = builder
        .pure(1 + (cursor.next_u64() % input.budget))
        .expect("u64 amount");
    let opaque_bytes_arg =
        builder.pure_bytes(cursor.next_bytes(8), /* force_separate */ false);

    let mut input_args = vec![recipient_arg, sender_arg, amount_arg, opaque_bytes_arg];
    let mut object_args = Vec::with_capacity(coin_refs.len());
    for oref in coin_refs {
        let arg = builder
            .obj(ObjectArg::ImmOrOwnedObject(*oref))
            .expect("fixture objects should be valid PTB inputs");
        input_args.push(arg);
        object_args.push(arg);
    }

    let mut result_args = Vec::new();
    for _ in 0..cursor.next_usize(MAX_COMMANDS) {
        let result = match cursor.next_u8() % 5 {
            0 => builder.command(build_known_transfer_move_call(
                cursor,
                &object_args,
                recipient_arg,
            )),
            1 => builder.command(build_fuzzed_move_call(cursor, &input_args, &result_args)),
            2 => builder.command(build_make_move_vec(cursor, &input_args, &result_args)),
            3 => builder.command(Command::Publish(
                build_module_blobs(cursor),
                build_dependency_ids(cursor),
            )),
            _ => {
                let ticket = if result_args.is_empty() {
                    sender_arg
                } else {
                    result_args[cursor.next_usize(result_args.len())]
                };
                builder.command(Command::Upgrade(
                    build_module_blobs(cursor),
                    build_dependency_ids(cursor),
                    select_package_id(cursor),
                    ticket,
                ))
            }
        };
        result_args.push(result);
    }

    builder.finish()
}

fn run_linkage_context_case(data: &[u8]) {
    let mut cursor = Cursor::new(data);
    let input = parse_input(&mut cursor);
    let primary = Object::new_gas_with_balance_and_owner_for_testing(
        input.primary_coin_balance,
        input.sender,
    );
    let secondary = Object::new_gas_with_balance_and_owner_for_testing(
        input.secondary_coin_balance,
        input.sender,
    );
    let coin_refs = [
        primary.compute_object_reference(),
        secondary.compute_object_reference(),
    ];

    let backing_store = framework_store();
    let checked_inputs = CheckedInputObjects::new_for_replay(InputObjects::new(vec![
        ObjectReadResult::new_from_gas_object(&primary),
        ObjectReadResult::new_from_gas_object(&secondary),
    ]));
    let mut temporary_store = TemporaryStore::new(
        &backing_store,
        checked_inputs.into_inner(),
        vec![],
        input.tx_digest,
        &STANDARD_CONFIG,
        EPOCH_ID,
    );

    let txn = build_transaction(&mut cursor, input, &coin_refs);
    let mut gas_charger = GasCharger::new_unmetered(input.tx_digest);
    let gas_payment = gas_charger.gas_payment_amount();
    let linkage_analysis =
        LinkageAnalyzer::new::<execution_mode::Normal>(&STANDARD_CONFIG).expect("linkage analysis");
    let package_store = CachedPackageStore::new(
        STANDARD_RUNTIME.as_ref(),
        TransactionPackageStore::new(&backing_store),
    );

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
    let Ok(resolution_vm) = STANDARD_RUNTIME
        .as_ref()
        .make_vm(&package_store.package_store, linkage_context)
    else {
        return;
    };

    let env = Env::new(
        &STANDARD_CONFIG,
        STANDARD_RUNTIME.as_ref(),
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
        &STANDARD_CONFIG,
    )));
    let mut translation_meter =
        translation_meter::TranslationMeter::new(&STANDARD_CONFIG, &mut gas_charger);

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

    let _ = typing::translate_and_verify::<execution_mode::Normal>(
        &mut translation_meter,
        &env,
        loaded_tx,
    );
}

fuzz_target!(|data: &[u8]| {
    run_linkage_context_case(data);
});
