// Copyright (c) Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

#![no_main]

use bcs::from_bytes;
use libfuzzer_sys::fuzz_target;
use sui_types::transaction::{CallArg, Command, ObjectArg, ProgrammableTransaction};

fuzz_target!(|data: &[u8]| {
    if let Ok(pt) = from_bytes::<ProgrammableTransaction>(data) {
        for input in &pt.inputs {
            match input {
                CallArg::Pure(bytes) => {
                    let _ = bytes.clone();
                }
                CallArg::Object(obj) => match obj {
                    ObjectArg::ImmOrOwnedObject(oref) => {
                        let _ = oref.0;
                    }
                    ObjectArg::SharedObject { id, .. } => {
                        let _ = *id;
                    }
                    ObjectArg::Receiving(oref) => {
                        let _ = oref.0;
                    }
                },
                CallArg::FundsWithdrawal(f) => {
                    let _ = f.reservation;
                    let _ = f.type_arg.clone();
                }
            }
        }
        for cmd in &pt.commands {
            match cmd {
                Command::MoveCall(mc) => {
                    let _ = mc.package;
                    let _ = mc.module.clone();
                    let _ = mc.function.clone();
                }
                Command::TransferObjects(objs, addr) => {
                    let _ = objs.len();
                    let _ = *addr;
                }
                Command::SplitCoins(coin, amounts) => {
                    let _ = *coin;
                    let _ = amounts.len();
                }
                Command::MergeCoins(target, coins) => {
                    let _ = *target;
                    let _ = coins.len();
                }
                Command::MakeMoveVec(ty, args) => {
                    let _ = ty.clone();
                    let _ = args.len();
                }
                Command::Publish(_, _) => {}
                Command::Upgrade(_, _, _, _) => {}
            }
        }
    }

    if let Ok(call_arg) = from_bytes::<CallArg>(data) {
        let _ = call_arg;
    }
});
