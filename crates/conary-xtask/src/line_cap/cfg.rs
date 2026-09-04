// crates/conary-xtask/src/line_cap/cfg.rs

use quote::ToTokens;
use std::collections::BTreeMap;
use syn::{Attribute, Meta, Token, parse::Parser, punctuated::Punctuated};

enum Predicate {
    Atom(String),
    All(Vec<Self>),
    Any(Vec<Self>),
    Not(Box<Self>),
}

impl Predicate {
    fn parse(meta: Meta) -> Option<Self> {
        let Meta::List(list) = meta else {
            return Some(Self::Atom(meta.to_token_stream().to_string()));
        };
        let children = Punctuated::<Meta, Token![,]>::parse_terminated
            .parse2(list.tokens)
            .ok()?
            .into_iter()
            .map(Self::parse)
            .collect::<Option<Vec<_>>>()?;
        if list.path.is_ident("all") {
            Some(Self::All(children))
        } else if list.path.is_ident("any") {
            Some(Self::Any(children))
        } else if list.path.is_ident("not") && children.len() == 1 {
            Some(Self::Not(Box::new(children.into_iter().next()?)))
        } else {
            None
        }
    }

    fn evaluate(&self, values: &BTreeMap<String, bool>) -> Option<bool> {
        match self {
            Self::Atom(atom) => values.get(atom).copied(),
            Self::Not(child) => child.evaluate(values).map(|value| !value),
            Self::All(children) | Self::Any(children) => {
                let identity = matches!(self, Self::All(_));
                let mut unknown = false;
                for child in children {
                    match child.evaluate(values) {
                        Some(value) if value != identity => return Some(value),
                        None => unknown = true,
                        _ => {}
                    }
                }
                (!unknown).then_some(identity)
            }
        }
    }

    fn unassigned_atom<'a>(&'a self, values: &BTreeMap<String, bool>) -> Option<&'a str> {
        match self {
            Self::Atom(atom) => (!values.contains_key(atom)).then_some(atom),
            Self::Not(child) => child.unassigned_atom(values),
            Self::All(children) | Self::Any(children) => children
                .iter()
                .find_map(|child| child.unassigned_atom(values)),
        }
    }

    // Shannon expansion preserves repeated-atom identity and proves satisfiability
    // across every platform/feature assignment, without assuming this host's cfg.
    fn satisfiable(&self, values: &mut BTreeMap<String, bool>) -> bool {
        if let Some(value) = self.evaluate(values) {
            return value;
        }
        let atom = self
            .unassigned_atom(values)
            .expect("unknown predicate has an unassigned atom")
            .to_owned();
        for value in [false, true] {
            values.insert(atom.clone(), value);
            if self.satisfiable(values) {
                values.remove(&atom);
                return true;
            }
        }
        values.remove(&atom);
        false
    }
}

pub(super) fn is_test_only(attributes: &[Attribute]) -> bool {
    let mut predicates = Vec::new();
    for attribute in attributes {
        let Some(predicate) = effective_cfg(&attribute.meta) else {
            return false;
        };
        predicates.push(predicate);
    }
    let predicate = Predicate::All(predicates);
    let mut values = BTreeMap::from([("test".to_owned(), false)]);
    if !predicate.satisfiable(&mut values) {
        // Never compiled outside test builds: test-only when some test build keeps it.
        values.insert("test".to_owned(), true);
        return predicate.satisfiable(&mut values);
    }

    // The node reaches some non-test build. rustc strips test-annotated functions
    // from non-test output, so the node is test-only only when every non-test
    // configuration that compiles it also applies a test annotation. A conditional
    // annotation whose condition can be false leaves an ordinary production item.
    let annotations = attributes
        .iter()
        .filter_map(|attribute| test_annotation(&attribute.meta))
        .collect();
    let retains_production = Predicate::All(vec![
        predicate,
        Predicate::Not(Box::new(Predicate::Any(annotations))),
    ]);
    !retains_production.satisfiable(&mut values)
}

fn effective_cfg(meta: &Meta) -> Option<Predicate> {
    if meta.path().is_ident("cfg") {
        let Meta::List(list) = meta else { return None };
        return Predicate::parse(syn::parse2(list.tokens.clone()).ok()?);
    }
    if meta.path().is_ident("cfg_attr") {
        let Meta::List(list) = meta else { return None };
        let mut arguments = Punctuated::<Meta, Token![,]>::parse_terminated
            .parse2(list.tokens.clone())
            .ok()?
            .into_iter();
        let condition = Predicate::parse(arguments.next()?)?;
        let nested = arguments
            .map(|meta| effective_cfg(&meta))
            .collect::<Option<Vec<_>>>()?;
        // When cfg_attr is inactive it imposes no restriction: condition implies
        // all introduced cfg predicates, including recursively nested cfg_attr.
        return Some(Predicate::Any(vec![
            Predicate::Not(Box::new(condition)),
            Predicate::All(nested),
        ]));
    }
    Some(Predicate::All(Vec::new()))
}

fn test_annotation(meta: &Meta) -> Option<Predicate> {
    if meta
        .path()
        .segments
        .last()
        .is_some_and(|segment| segment.ident == "test")
    {
        return Some(Predicate::All(Vec::new()));
    }
    let Meta::List(list) = meta else {
        return None;
    };
    if !list.path.is_ident("cfg_attr") {
        return None;
    }
    let mut arguments = Punctuated::<Meta, Token![,]>::parse_terminated
        .parse2(list.tokens.clone())
        .ok()?
        .into_iter();
    let condition = Predicate::parse(arguments.next()?)?;
    let annotations = arguments
        .filter_map(|meta| test_annotation(&meta))
        .collect();
    Some(Predicate::All(vec![condition, Predicate::Any(annotations)]))
}
