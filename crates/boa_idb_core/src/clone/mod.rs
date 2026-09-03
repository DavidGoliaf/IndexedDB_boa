//! Structured clone value representation and SCF-v1 binary codec.
//!
//! `ScValue` is an intermediate representation of JavaScript values that is
//! independent of any JS engine. The SCF-v1 codec serializes/deserializes
//! `ScValue` to/from a compact binary format with CRC32C integrity checks.

pub mod decode;
pub mod encode;
pub mod scvalue;
pub mod varint;
