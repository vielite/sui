// Copyright (c) Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

#![no_main]

use bcs::from_bytes;
use libfuzzer_sys::fuzz_target;
use sui_types::transaction::ProgrammableTransaction;

fuzz_target!(|data: &[u8]| {
    let _ = from_bytes::<ProgrammableTransaction>(data);
});
