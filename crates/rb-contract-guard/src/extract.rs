//! Wire-shape extraction: parse a Rust source file and digest the items that
//! define the serialized contract.
//!
//! What counts as shape-defining (and is digested):
//! - `struct` and `enum` definitions, including all their attributes — serde
//!   attrs (`tag`, `default`, `skip_serializing_if`, `rename_all`, ...) ARE the
//!   wire shape;
//! - explicit `impl Serialize/Deserialize for T` blocks (none exist today; the
//!   guard must not go blind if one appears);
//! - the `CONTRACT_VERSION` const.
//!
//! What is normalized away (never trips the guard):
//! - doc comments (`///`, `#[doc = ...]`) anywhere on/inside an item;
//! - regular comments and formatting (dropped by parsing + token printing);
//! - functions, inherent impls, other consts, `use` items, `#[cfg(test)]` items.

use anyhow::{bail, Result};
use quote::ToTokens;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use syn::{Attribute, Fields, ImplItem, Item, ItemImpl};

/// Hex-encoded sha256 of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

fn is_doc(attr: &Attribute) -> bool {
    attr.path().is_ident("doc")
}

fn is_cfg_test(attrs: &[Attribute]) -> bool {
    attrs
        .iter()
        .any(|a| a.path().is_ident("cfg") && a.to_token_stream().to_string().contains("test"))
}

fn strip_docs(attrs: &mut Vec<Attribute>) {
    attrs.retain(|a| !is_doc(a));
}

fn strip_docs_in_fields(fields: &mut Fields) {
    for field in fields.iter_mut() {
        strip_docs(&mut field.attrs);
    }
}

/// The trait name when `imp` is an explicit serde impl (`Serialize` or
/// `Deserialize`), else `None`.
fn serde_impl_trait(imp: &ItemImpl) -> Option<String> {
    let (_, path, _) = imp.trait_.as_ref()?;
    let last = path.segments.last()?.ident.to_string();
    (last == "Serialize" || last == "Deserialize").then_some(last)
}

fn digest_tokens(tokens: &impl ToTokens) -> String {
    sha256_hex(tokens.to_token_stream().to_string().as_bytes())
}

/// Extract `"<rel_path>::<item name>"` -> digest for every shape-defining item
/// in `source`.
pub fn extract_wire_items(rel_path: &str, source: &str) -> Result<BTreeMap<String, String>> {
    let file: syn::File = match syn::parse_file(source) {
        Ok(f) => f,
        Err(e) => bail!("failed to parse {rel_path} as Rust: {e}"),
    };

    let mut out = BTreeMap::new();
    for item in file.items {
        match item {
            Item::Struct(mut s) => {
                if is_cfg_test(&s.attrs) {
                    continue;
                }
                strip_docs(&mut s.attrs);
                strip_docs_in_fields(&mut s.fields);
                out.insert(format!("{rel_path}::{}", s.ident), digest_tokens(&s));
            }
            Item::Enum(mut e) => {
                if is_cfg_test(&e.attrs) {
                    continue;
                }
                strip_docs(&mut e.attrs);
                for variant in &mut e.variants {
                    strip_docs(&mut variant.attrs);
                    strip_docs_in_fields(&mut variant.fields);
                }
                out.insert(format!("{rel_path}::{}", e.ident), digest_tokens(&e));
            }
            Item::Const(mut c) => {
                if is_cfg_test(&c.attrs) || c.ident != "CONTRACT_VERSION" {
                    continue;
                }
                strip_docs(&mut c.attrs);
                out.insert(format!("{rel_path}::{}", c.ident), digest_tokens(&c));
            }
            Item::Impl(mut imp) => {
                if is_cfg_test(&imp.attrs) {
                    continue;
                }
                let Some(trait_name) = serde_impl_trait(&imp) else {
                    continue;
                };
                strip_docs(&mut imp.attrs);
                for inner in &mut imp.items {
                    if let ImplItem::Fn(f) = inner {
                        strip_docs(&mut f.attrs);
                    }
                }
                let ty = imp.self_ty.to_token_stream().to_string();
                out.insert(
                    format!("{rel_path}::impl {trait_name} for {ty}"),
                    digest_tokens(&imp),
                );
            }
            _ => {}
        }
    }
    Ok(out)
}

/// Parse the integer value of `const CONTRACT_VERSION: u32 = N;` in `source`.
pub fn parse_contract_version(source: &str) -> Result<u32> {
    let file: syn::File = match syn::parse_file(source) {
        Ok(f) => f,
        Err(e) => bail!("failed to parse version file as Rust: {e}"),
    };
    for item in file.items {
        let Item::Const(c) = item else { continue };
        if c.ident != "CONTRACT_VERSION" {
            continue;
        }
        let syn::Expr::Lit(lit) = c.expr.as_ref() else {
            bail!("CONTRACT_VERSION is not a literal expression");
        };
        let syn::Lit::Int(int) = &lit.lit else {
            bail!("CONTRACT_VERSION is not an integer literal");
        };
        return Ok(int.base10_parse::<u32>()?);
    }
    bail!("no `const CONTRACT_VERSION` found in version file")
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = r#"
        /// Wire contract version.
        pub const CONTRACT_VERSION: u32 = 2;

        /// First frame the client sends.
        #[derive(Serialize, Deserialize)]
        pub struct Handshake {
            /// Negotiated version.
            pub contract_version: u32,
            #[serde(default)]
            pub identity: Option<String>,
        }

        /// One request per op.
        #[derive(Serialize, Deserialize)]
        #[serde(tag = "op")]
        pub enum Request {
            Ping,
            Get { id: String },
        }

        pub const SOME_TUNING_KNOB: f32 = 0.18;

        pub fn helper() {}

        impl Handshake {
            pub fn inherent(&self) {}
        }

        #[cfg(test)]
        mod tests {
            pub struct NotWire;
        }
    "#;

    fn items(src: &str) -> BTreeMap<String, String> {
        extract_wire_items("f.rs", src).unwrap()
    }

    #[test]
    fn extracts_structs_enums_and_contract_version_only() {
        let map = items(BASE);
        let keys: Vec<&str> = map.keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            vec!["f.rs::CONTRACT_VERSION", "f.rs::Handshake", "f.rs::Request"],
            "only shape-defining items are digested"
        );
        for digest in map.values() {
            assert_eq!(digest.len(), 64, "sha256 hex digest");
            assert!(digest.chars().all(|c| c.is_ascii_hexdigit()));
        }
    }

    #[test]
    fn doc_comments_and_formatting_do_not_change_digests() {
        let reworded = r#"
        // A regular comment the parser drops.
        pub const CONTRACT_VERSION: u32 = 2;
        #[derive(Serialize, Deserialize)]
        pub struct Handshake {
                pub contract_version: u32,
            #[serde(default)] pub identity: Option<String>,
        }
        /// Entirely different documentation prose.
        #[derive(Serialize, Deserialize)]
        #[serde(tag = "op")]
        pub enum Request { Ping, Get { id: String }, }
        pub const SOME_TUNING_KNOB: f32 = 0.18;
        pub fn helper() {}
        impl Handshake { pub fn inherent(&self) {} }
        #[cfg(test)]
        mod tests { pub struct NotWire; }
        "#;
        assert_eq!(
            items(BASE),
            items(reworded),
            "docs/comments/whitespace are not part of the wire shape"
        );
    }

    #[test]
    fn adding_a_field_changes_the_digest() {
        let added = BASE.replace(
            "pub identity: Option<String>,",
            "pub identity: Option<String>,\n pub extra: Option<u8>,",
        );
        let before = items(BASE);
        let after = items(&added);
        assert_ne!(
            before.get("f.rs::Handshake"),
            after.get("f.rs::Handshake"),
            "a new field must change the item digest"
        );
        assert_eq!(
            before.get("f.rs::Request"),
            after.get("f.rs::Request"),
            "unrelated items keep their digest"
        );
    }

    #[test]
    fn changing_a_serde_attribute_changes_the_digest() {
        let retagged = BASE.replace("#[serde(tag = \"op\")]", "#[serde(tag = \"operation\")]");
        assert_ne!(
            items(BASE).get("f.rs::Request"),
            items(&retagged).get("f.rs::Request"),
            "serde attrs are part of the wire shape"
        );
    }

    #[test]
    fn adding_an_enum_variant_changes_the_digest() {
        let grown = BASE.replace("Get { id: String },", "Get { id: String },\n Stats,");
        assert_ne!(
            items(BASE).get("f.rs::Request"),
            items(&grown).get("f.rs::Request"),
            "a new variant must change the enum digest"
        );
    }

    #[test]
    fn custom_serde_impls_are_digested_but_inherent_impls_are_not() {
        let with_custom = r#"
            pub struct Weird;
            impl serde::Serialize for Weird {
                fn serialize<S>(&self, s: S) -> Result<S::Ok, S::Error> { s.serialize_unit() }
            }
            impl Weird { pub fn helper(&self) {} }
        "#;
        let map = items(with_custom);
        assert!(
            map.contains_key("f.rs::impl Serialize for Weird"),
            "explicit serde impls define wire shape; keys: {:?}",
            map.keys().collect::<Vec<_>>()
        );
        assert_eq!(map.len(), 2, "struct + serde impl, no inherent impl");
    }

    #[test]
    fn contract_version_value_is_parsed() {
        assert_eq!(parse_contract_version(BASE).unwrap(), 2);
    }

    #[test]
    fn missing_contract_version_errors() {
        let err = parse_contract_version("pub struct X;").unwrap_err();
        assert!(err.to_string().contains("CONTRACT_VERSION"));
    }

    #[test]
    fn invalid_rust_errors() {
        assert!(extract_wire_items("f.rs", "not rust at all {{{").is_err());
    }
}
