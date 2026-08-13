#![allow(
    mismatched_lifetime_syntaxes,
    dead_code,
    unused_imports,
    unused_variables,
    unreachable_code,
    unreachable_patterns,
    unreachable_pub
)]

use std::collections::{HashMap, HashSet};

use convert_case::Casing;
use proc_macro::TokenStream;
use proc_macro2::{Group, Punct, Spacing, Span, TokenTree};
use quote::{ToTokens, quote};
use syn::{
    DeriveInput, Fields, GenericArgument, GenericParam, Ident, Lifetime, TypePath, parse_macro_input, punctuated::Punctuated,
    token::Comma,
};

fn resolve_migrateable_path() -> proc_macro2::TokenStream {
    match proc_macro_crate::crate_name("migrateable") {
        Ok(proc_macro_crate::FoundCrate::Itself) => quote! { crate },
        Ok(proc_macro_crate::FoundCrate::Name(name)) => {
            let ident = syn::Ident::new(&name, proc_macro2::Span::call_site());
            quote! { ::#ident }
        }
        Err(_) => {
            let ident = syn::Ident::new("migrateable", proc_macro2::Span::call_site());
            quote! { ::#ident }
        }
    }
}

fn replace_lifetimes(tokens: proc_macro2::TokenStream, replacement: Ident) -> (proc_macro2::TokenStream, bool) {
    let mut iter = tokens.into_iter().peekable();
    let mut result = proc_macro2::TokenStream::new();
    let mut changed = false;

    while let Some(token) = iter.next() {
        match token {
            TokenTree::Punct(ref p) if p.as_char() == '\'' && p.spacing() == Spacing::Joint => {
                if let Some(TokenTree::Ident(_)) = iter.peek() {
                    iter.next();
                    result
                        .extend(vec![TokenTree::Punct(Punct::new('\'', Spacing::Joint)), TokenTree::Ident(replacement.clone())]);
                    changed = true;
                } else {
                    result.extend(vec![token]);
                }
            }
            TokenTree::Group(group) => {
                let (new_stream, inner_changed) = replace_lifetimes(group.stream(), replacement.clone());
                changed = changed || inner_changed;
                result.extend(vec![TokenTree::Group(Group::new(group.delimiter(), new_stream))]);
            }
            _ => result.extend(vec![token]),
        }
    }
    (result, changed)
}

#[derive(Copy, Clone, Hash, Eq, PartialEq, strum_macros::EnumString, strum_macros::IntoStaticStr)]
enum MigrateableType {
    Type,
    KeyType,
    RkyvType,
    JsonType,
    BincodeType,
    MsgpackType,
}

#[proc_macro_derive(Migrateable)]
pub fn derive_migrateable(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let migrateable_path = resolve_migrateable_path();

    let ident = &input.ident;
    let generics = &input.generics;

    let syn::Data::Enum(data_enum) = &input.data else {
        panic!("Migrateable can only be derived for enums");
    };
    let non_lifetime_params: Vec<_> = generics.params.iter().filter(|p| !matches!(p, GenericParam::Lifetime(_))).collect();

    let impl_generics = if non_lifetime_params.is_empty() {
        quote! {}
    } else {
        quote! { < #(#non_lifetime_params),* > }
    };

    let where_clause = &generics.where_clause;

    let build_args = |replacement: proc_macro2::TokenStream| -> proc_macro2::TokenStream {
        if generics.params.is_empty() {
            return quote! {};
        }
        let args: Vec<_> = generics
            .params
            .iter()
            .map(|param| match param {
                GenericParam::Lifetime(_) => replacement.clone(),
                GenericParam::Type(t) => {
                    let ident = &t.ident;
                    quote! { #ident }
                }
                GenericParam::Const(c) => {
                    let ident = &c.ident;
                    quote! { #ident }
                }
            })
            .collect();
        quote! { < #(#args),* > }
    };

    let marker_args = build_args(quote! { 'static });

    let all_variants: Vec<_> = data_enum.variants.iter().map(|v| &v.ident).collect();
    let mut all_variant_types: Vec<_> = Vec::new();
    let mut last_variant = None;
    let mut last_variant_type = None;
    let mut last_variant_type_current = None;
    let mut last_variant_type_local = None;
    let mut last_variant_migrate_serializer = None;
    let mut last_variant_migrate_deserializer = None;
    let mut last_variant_type_current_lifetime = None;
    let mut migrating_variants: Vec<_> = Vec::new();
    let mut migrating_variant_types: Vec<_> = Vec::new();
    let mut migrating_variant_migrate_deserializers: Vec<_> = Vec::new();

    let mut variant_data_kinds = Vec::new();

    let mut used_variant_types = HashSet::new();
    let type_trait_name = generate_trait_name_for_variant(ident, MigrateableType::Type);

    for (index, variant) in data_enum.variants.iter().enumerate() {
        let Some(field) = &variant.fields.iter().next() else {
            panic!("Migrateable can only be derived for enums with tuple variants containing exactly one field");
        };

        let syn::Type::Path(serialization_wrapper) = &field.ty else {
            panic!(
                "Migrateable enum variants must have a single field matching one of 'Type', 'RkyvType', 'JsonType', 'BincodeType', or 'MsgpackType'"
            );
        };

        let segments = &serialization_wrapper.path.segments;
        let Some(bounds_segment) = segments.last() else {
            panic!("Migrateable tuple variants must hold a type");
        };
        let syn::PathArguments::AngleBracketed(type_bounds) = &bounds_segment.arguments else {
            panic!("Migrateable type variants must have a type argument");
        };
        if type_bounds.args.len() != 1 {
            panic!("Migrateable type variants must have exactly one type argument");
        }

        let mut serialization_wrapper = serialization_wrapper.clone();
        let last_segment = serialization_wrapper.path.segments.last_mut().unwrap();
        last_segment.arguments = syn::PathArguments::None;

        let variant_type = last_segment.ident.to_string().parse::<MigrateableType>().expect("Unknown migrateable type");
        match variant_type {
            MigrateableType::Type => variant_data_kinds.push(quote! { #migrateable_path::MigrateDataKind::Bytes }),
            MigrateableType::KeyType => variant_data_kinds.push(quote! { #migrateable_path::MigrateDataKind::Key }),
            MigrateableType::RkyvType => variant_data_kinds.push(quote! { #migrateable_path::MigrateDataKind::Rkyv }),
            MigrateableType::JsonType => variant_data_kinds.push(quote! { #migrateable_path::MigrateDataKind::Json }),
            MigrateableType::BincodeType => variant_data_kinds.push(quote! { #migrateable_path::MigrateDataKind::Bincode }),
            MigrateableType::MsgpackType => variant_data_kinds.push(quote! { #migrateable_path::MigrateDataKind::Msgpack }),
        }
        used_variant_types.insert(variant_type);
        let variant_ident = variant.ident.clone();

        let variant_inner_type = match &type_bounds.args[0] {
            syn::GenericArgument::Type(inner_type) => inner_type.clone(),
            _ => panic!("Expected type argument"),
        };

        let variant_deserializer = match variant_type {
            MigrateableType::Type => quote! { #type_trait_name::deserialize(serialize_bound, bytes.as_ref())? },
            MigrateableType::KeyType => {
                quote! { ::storekey::decode_borrow::<'_, <#variant_inner_type as fjall_expanded::Key>::Borrowed<'_>>(bytes.as_ref()).map_err(|e| #migrateable_path::migrate_error!(e))? }
            }
            MigrateableType::RkyvType => quote! {
                {
                if cfg!(debug_assertions) {
                    ::rkyv::from_bytes::<#variant_inner_type, ::rkyv::rancor::Error>(bytes.as_ref())?
                } else {
                    unsafe { ::rkyv::from_bytes_unchecked::<#variant_inner_type, ::rkyv::rancor::Error>(bytes.as_ref())? }
                }
                }
            },
            MigrateableType::JsonType => quote! { ::serde_json::from_slice(bytes.as_ref())? },
            MigrateableType::BincodeType => quote! { ::bincode::deserialize(bytes.as_ref())? },
            MigrateableType::MsgpackType => quote! { ::zerompk::from_msgpack(bytes.as_ref())? },
        };

        all_variant_types.push(type_bounds.args[0].clone());
        if index < data_enum.variants.len() - 1 {
            migrating_variants.push(variant_ident);
            migrating_variant_types
                .push(replace_lifetimes(variant_inner_type.into_token_stream(), Ident::new("_", Span::call_site())).0);
            migrating_variant_migrate_deserializers.push(variant_deserializer);
        } else {
            last_variant = Some(variant_ident);
            last_variant_migrate_serializer = Some(match variant_type {
                MigrateableType::Type => quote! { #type_trait_name::serialize(serialize_bound, migrated)? },
                MigrateableType::KeyType => {
                    quote! { ::storekey::encode_vec(&migrated).map_err(|e| #migrateable_path::migrate_error!(e))? }
                }
                MigrateableType::RkyvType => {
                    quote! { ::rkyv::api::high::to_bytes_in::<Vec<u8>, ::rkyv::rancor::Error>(&migrated, Vec::<u8>::new())? }
                }
                MigrateableType::JsonType => quote! { ::serde_json::to_vec(&migrated)? },
                MigrateableType::BincodeType => quote! { ::bincode::serialize(&migrated)? },
                MigrateableType::MsgpackType => quote! { ::rmp_serde::to_vec(&migrated)? },
            });
            last_variant_migrate_deserializer = Some(variant_deserializer);
            last_variant_type = Some(variant_inner_type.clone());

            let (replaced_value, replaced) =
                replace_lifetimes(variant_inner_type.clone().into_token_stream(), Ident::new("c", Span::call_site()));

            if replaced {
                last_variant_type_current_lifetime = Some(quote! { <'c> });
            }

            last_variant_type_current = Some(replaced_value);
            last_variant_type_local =
                Some(replace_lifetimes(variant_inner_type.into_token_stream(), Ident::new("_", Span::call_site())).0);
        }
    }

    let type_serialization_trait = if used_variant_types.contains(&MigrateableType::Type) {
        let mut impls = quote! {
            trait #type_trait_name<'de, T> {
                fn serialize(&self, data: T) -> Result<Vec<u8>, #migrateable_path::MigrateError>;
                fn deserialize(&self, bytes: &'de [u8]) -> Result<T, #migrateable_path::MigrateError>;
            }

            impl<'de> #type_trait_name<'de, Vec<u8>> for std::marker::PhantomData<Vec<u8>> {
                fn serialize(&self, data: Vec<u8>) -> #migrateable_path::MigrateResult<Vec<u8>> {
                    Ok(data)
                }
                fn deserialize(&self, bytes: &'de [u8]) -> #migrateable_path::MigrateResult<Vec<u8>> {
                    Ok(bytes.to_vec())
                }
            }
            impl<'de, const N: usize> #type_trait_name<'de, [u8; N]> for std::marker::PhantomData<[u8; N]> {
                fn serialize(&self, data: [u8; N]) -> #migrateable_path::MigrateResult<Vec<u8>> {
                    Ok(data.to_vec())
                }
                fn deserialize(&self, bytes: &'de [u8]) -> #migrateable_path::MigrateResult<[u8; N]> {
                    if bytes.len() != N {
                        return Err(#migrateable_path::migrate_error!("Invalid data length"));
                    }
                    let mut arr = [0u8; N];
                    arr.copy_from_slice(bytes);
                    Ok(arr)
                }
            }
            impl<'de> #type_trait_name<'de, String> for std::marker::PhantomData<String> {
                fn serialize(&self, data: String) -> #migrateable_path::MigrateResult<Vec<u8>> {
                    Ok(data.into_bytes())
                }
                fn deserialize(&self, bytes: &'de [u8]) -> #migrateable_path::MigrateResult<String> {
                    Ok(String::from_utf8(bytes.to_vec()).map_err(|e| #migrateable_path::migrate_error!(e))?)
                }
            }
        };

        // if feature fjall then extend impls with ::fjall::Slice impl
        if cfg!(feature = "fjall") {
            impls.extend(quote! {
                impl<'de> #type_trait_name<'de, ::fjall::Slice> for std::marker::PhantomData::<::fjall::Slice> {
                    fn serialize(&self, data: ::fjall::Slice) -> #migrateable_path::MigrateResult<Vec<u8>> {
                        Ok(data.as_ref().to_vec())
                    }
                    fn deserialize(&self, bytes: &'de [u8]) -> #migrateable_path::MigrateResult::<::fjall::Slice> {
                        Ok(::fjall::Slice::new(bytes))
                    }
                }
            });
        }

        Some(impls)
    } else {
        None
    };

    let last_variant_data_kind = variant_data_kinds.last().expect("At least one variant must exist");

    let variant_ids: Vec<u8> = all_variants
        .iter()
        .map(|i| {
            let version_string = i.to_string();
            // get numbers from end that are digits
            let version_number = version_string
                .chars()
                .rev()
                .take_while(|c| c.is_ascii_digit())
                .collect::<String>()
                .chars()
                .rev()
                .collect::<String>();
            version_number.parse::<u8>().expect("Variant name must end with a number")
        })
        .collect();

    if variant_ids.len() != variant_ids.iter().collect::<HashSet<_>>().len() {
        panic!("Variant IDs must be unique");
    }

    for window in variant_ids.windows(2) {
        if window[0] >= window[1] {
            panic!("Variant IDs must be strictly increasing");
        }
    }

    let migrateable_variant_ids: Vec<_> = variant_ids.iter().take(migrating_variants.len()).collect();

    let last_variant_id = *variant_ids.last().expect("At least one variant must exist");

    let current_alias = quote::format_ident!("Current{}", ident);
    let current_alias_visibility = &input.vis;

    let migrateable_versions_enum = quote::format_ident!("Migrateable{ident}");

    let migrate_to_trait = syn::Ident::new(&format!("MigrateTo{}", current_alias), current_alias.span());

    let type_hash_method = match cfg!(feature = "hashed") {
        true => quote! {
            #[allow(unused_imports)]
            fn type_hash(&self) -> u128
            {
                use #migrateable_path::HashedTypeDef;
                use #migrateable_path::MigrateableHashed;
                match self {
                    #( #ident::#all_variants(_) => <#all_variant_types>::TYPE_HASH_NATIVE, )*
                }
            }
        },
        false => quote! {},
    };

    let expanded = quote! {
        #current_alias_visibility type #current_alias #last_variant_type_current_lifetime = #last_variant_type_current;

        trait #migrate_to_trait {
            fn migrate(self) -> #migrateable_path::MigrateResult<#last_variant_type>;
        }

        enum #migrateable_versions_enum {
            #(#migrating_variants,)*
        }

        #[automatically_derived]
        impl #migrateable_path::Migrateable for #ident #marker_args #where_clause {
            type Current = #last_variant_type;
            type CurrentLifetimed<'c> = #last_variant_type_current;

            const CURRENT_DATA_KIND: #migrateable_path::MigrateDataKind = #last_variant_data_kind;
            const CURRENT_VERSION: u8 = #last_variant_id;

            fn iter() -> impl Iterator<Item = Self> {
                vec![
                    #( #ident::#all_variants(std::marker::PhantomData), )*
                ].into_iter()
            }

            fn iter_migrateable() -> impl Iterator<Item = Self> {
                vec![
                    #( #ident::#migrating_variants(std::marker::PhantomData), )*
                ].into_iter()
            }

            fn iter_u8() -> impl Iterator<Item = u8> {
                vec![#(#variant_ids),*].into_iter()
            }

            fn iter_migrateable_u8() -> impl Iterator<Item = u8> {
                vec![#(#migrateable_variant_ids),*].into_iter()
            }

            fn from_u8(value: u8) -> Option<Self> {
                match value {
                    #( #variant_ids => Some(#ident::#all_variants(std::marker::PhantomData)), )*
                    _ => None,
                }
            }

            fn to_u8(&self) -> u8 {
                match self {
                    #( #ident::#all_variants(_) => #variant_ids, )*
                }
            }

            fn data_kind(&self) -> #migrateable_path::MigrateDataKind {
                match self {
                    #( #ident::#all_variants(_) => #variant_data_kinds, )*
                }
            }

            #type_hash_method

            fn migrate<'de, I: 'de + AsRef<[u8]>, O: From<Vec<u8>>>(&self, bytes: I) -> Result<O, #migrateable_path::MigrateError>
            {
                Ok(match self {
                    #( #ident::#migrating_variants(serialize_bound) => {
                        let data: #migrating_variant_types = if #migrateable_path::typeid::of::<I>() == #migrateable_path::typeid::of::<#migrating_variant_types>() {
                            unsafe {
                                let data = std::mem::transmute_copy::<I, #migrating_variant_types>(&bytes);
                                std::mem::forget(bytes);
                                data
                            }
                        } else {
                            #migrating_variant_migrate_deserializers
                        };

                        let migrated: #last_variant_type_local = #migrate_to_trait::migrate(data)?;
                        let bound: std::marker::PhantomData<#last_variant_type_local> = std::marker::PhantomData;
                        let serialize_bound = &bound;
                        #last_variant_migrate_serializer.into()
                    }, )*
                    #ident::#last_variant(serialize_bound) => {
                        let migrated: #last_variant_type_local = #last_variant_migrate_deserializer;

                        #last_variant_migrate_serializer.into()
                    }
                })
            }
        }

        #type_serialization_trait
    };

    TokenStream::from(expanded)
}

fn generate_trait_name_for_variant(target_enum_ident: &syn::Ident, variant_type: MigrateableType) -> Ident {
    let variant_type_str: &'static str = variant_type.into();
    quote::format_ident!("Serializeable{}{}", target_enum_ident, variant_type_str)
}

#[cfg(feature = "hashed")]
fn parse_u128_lit(lit: &syn::LitInt) -> u128 {
    let s = lit.to_string().replace('_', "");
    let s = s
        .trim_end_matches("u128")
        .trim_end_matches("u64")
        .trim_end_matches("u32")
        .trim_end_matches("i128")
        .trim_end_matches("i64")
        .trim_end_matches("i32");
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u128::from_str_radix(hex, 16).expect("Invalid hex hash in #[locked]")
    } else if let Some(bin) = s.strip_prefix("0b").or_else(|| s.strip_prefix("0B")) {
        u128::from_str_radix(bin, 2).expect("Invalid binary hash in #[locked]")
    } else if let Some(oct) = s.strip_prefix("0o").or_else(|| s.strip_prefix("0O")) {
        u128::from_str_radix(oct, 8).expect("Invalid octal hash in #[locked]")
    } else {
        s.parse().expect("Invalid hash literal in #[locked]")
    }
}

/// Derive macro that locks a type's definition hash at compile time.
///
/// Requires that the type also derives `HashedTypeDef` (re-exported from `migrateable`).
/// Use `#[locked(0x...)]` to set the expected hash. A hash of `0x0` skips the check.
/// Optionally provide a message: `#[locked(0x..., "reason")]`.
///
/// The compile-time assertion is suppressed when the `__lock_discover` feature is active
/// (set internally by the `migrateable lock` CLI via `--features`).
#[cfg(feature = "hashed")]
#[proc_macro_derive(Locked, attributes(locked))]
pub fn derive_locked(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let migrateable_path = resolve_migrateable_path();
    let ident = &input.ident;
    let ident_str = ident.to_string();

    let has_type_params = input.generics.params.iter().any(|p| !matches!(p, GenericParam::Lifetime(_)));
    if has_type_params {
        return syn::Error::new_spanned(ident, "Locked cannot be derived on generic types (non-lifetime parameters)")
            .to_compile_error()
            .into();
    }

    let locked_args = input.attrs.iter().find_map(|attr| {
        if !attr.path().is_ident("locked") {
            return None;
        }
        let tokens: proc_macro2::TokenStream =
            attr.parse_args().expect("#[locked] requires a hash argument, e.g. #[locked(0x0)]");
        let mut iter = tokens.into_iter();
        let hash_token = iter.next().expect("#[locked] requires a hash argument");
        let hash_lit: syn::LitInt = syn::parse2(hash_token.into()).expect("#[locked] first argument must be a hash literal");
        let hash = parse_u128_lit(&hash_lit);
        let msg: Option<syn::LitStr> = if iter.next().is_some() {
            let rest: proc_macro2::TokenStream = iter.collect();
            Some(syn::parse2(rest).expect("#[locked] message must be a string literal"))
        } else {
            None
        };
        Some((hash, msg))
    });
    let locked_hash = locked_args.as_ref().map(|(h, _)| *h);
    let locked_msg = locked_args.and_then(|(_, m)| m);

    let lock_discover = cfg!(feature = "__lock_discover");

    let check = match locked_hash {
        Some(h) if h != 0 && !lock_discover => {
            let hash_lit = syn::LitInt::new(&format!("{h:#034x}u128"), proc_macro2::Span::call_site());
            let msg = if let Some(lit) = &locked_msg {
                quote! { concat!("Locked hash mismatch for `", #ident_str, "`.\n \n", #lit, "\n \nRun `cargo install migrateable && migrateable lock --overwrite` to update hash.\n ") }
            } else {
                quote! { concat!("Locked hash mismatch for `", #ident_str, "`.\n \nRun `cargo install migrateable && migrateable lock --overwrite` to update hash.\n ") }
            };
            quote! {
                const _: () = {
                    assert!(
                        <#ident as #migrateable_path::HashedTypeDef>::TYPE_HASH_NATIVE == #hash_lit,
                        #msg,
                    );
                };
            }
        }
        _ => quote! {},
    };

    let test = if lock_discover {
        let test_mod = quote::format_ident!("__migrateable_locked_{}", ident);
        quote! {
            #[cfg(test)]
            #[allow(non_snake_case)]
            mod #test_mod {
                #[test]
                fn migrateable_locked_hash() {
                    fn __get<T: #migrateable_path::HashedTypeDef>(
                        _: core::marker::PhantomData<T>,
                    ) -> u128 {
                        T::TYPE_HASH_NATIVE
                    }
                    let hash = __get::<super::#ident>(core::marker::PhantomData);
                    println!("MIGRATEABLE_LOCK:{}:{}:{:#034x}", file!(), #ident_str, hash);
                }
            }
        }
    } else {
        quote! {}
    };

    TokenStream::from(quote! { #check #test })
}
