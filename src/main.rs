mod symbols;

use std::ops::Range;

use eyre::{eyre, Context, Result};
use object::{File, Object, ObjectSection, ObjectSymbol};
use rustc_hash::FxHashMap;
use serde::Serialize;

use crate::symbols::symbol_components;

#[derive(serde::Serialize)]
struct SerGroup {
    id: String,
    label: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    groups: Vec<SerGroup>,
}

fn main() -> Result<()> {
    let path = std::env::args()
        .skip(1)
        .filter(|arg| !arg.starts_with("-"))
        .next()
        .unwrap_or("./target/debug/my-binary-is-thicc-af".into());

    let rodata = std::env::args().any(|arg| arg == "--rodata");

    let data = std::fs::read(&path).wrap_err_with(|| format!("error opening `{path}`"))?;
    let object = object::File::parse(data.as_slice()).context("could not parse object file")?;

    if !rodata {
        analyze_sym_modules(object)?;
    } else {
        let all_sections = object
            .sections()
            .filter(|section| section.name().is_ok_and(|name| name.contains(".rodata")))
            .map(|section| {
                let range = section.address()..(section.address() + section.size());
                symbol_sizes_in(&object, range)
            })
            .collect::<Result<Vec<_>>>()?;
        let mut symbol_sizes = all_sections.into_iter().flatten().collect::<Vec<_>>();
        symbol_sizes.sort_by_key(|&(_, size)| size);

        if symbol_sizes.is_empty() {
            eprintln!("no symbols found");
        } else {
            for (sym, size) in symbol_sizes {
                println!("{:10} - {}", size.to_string(), sym);
            }
        }
    }

    Ok(())
}

fn analyze_sym_modules(object: File<'_>) -> Result<()> {
    let limit = 100;

    let text = object
        .section_by_name(".text")
        .ok_or_else(|| eyre!("could not find .text section"))?;

    let text_range = text.address()..(text.address() + text.size());

    let symbol_sizes = symbol_sizes_in(&object, text_range)?;

    let mut root_groups = Groups(FxHashMap::default());

    for (sym, size) in symbol_sizes {
        let mut components = symbol_components(sym).with_context(|| sym.to_string())?;
        if components.len() > limit {
            components.truncate(limit);
        }

        eprintln!("{}", rustc_demangle::demangle(sym).to_string());

        add_to_group(&mut root_groups, components, size);
    }

    root_groups.0.values_mut().for_each(|g| {
        propagate_weight(g);
    });

    println!(
        "{}",
        serde_json::to_string(&root_groups).wrap_err("failed to serialize groups")?
    );
    Ok(())
}

#[derive(Debug)]
struct Groups(FxHashMap<String, Group>);

#[derive(Debug)]
struct Group {
    weight: u64,
    children: Groups,
}

impl Serialize for Groups {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeSeq;

        #[derive(Serialize)]
        struct ChildGroup<'a> {
            id: &'a str,
            label: &'a str,
            weight: u64,
            groups: &'a Groups,
        }

        let mut seq = serializer.serialize_seq(Some(self.0.len()))?;

        for (name, grp) in &self.0 {
            seq.serialize_element(&ChildGroup {
                id: name,
                label: name,
                weight: grp.weight,
                groups: &grp.children,
            })?;
        }

        seq.end()
    }
}

fn add_to_group(mut cur_groups: &mut Groups, components: Vec<String>, sym_size: u64) {
    for head in components {
        let grp = cur_groups.0.entry(head).or_insert(Group {
            weight: sym_size, // NOTE: This is a dummy value for everything but the innermost nesting.
            children: Groups(FxHashMap::default()),
        });
        cur_groups = &mut grp.children;
    }
}

fn propagate_weight(group: &mut Group) -> u64 {
    if group.children.0.is_empty() {
        return group.weight;
    }
    let total_weight: u64 = group.children.0.values_mut().map(propagate_weight).sum();
    group.weight = total_weight;
    total_weight
}

fn symbol_sizes_in<'a>(object: &'a File<'a>, range: Range<u64>) -> Result<Vec<(&'a str, u64)>> {
    let symbols = object
        .symbols()
        .into_iter()
        .filter(|sym| range.contains(&sym.address()))
        .collect::<Vec<_>>();

    let mut symbol_sizes = Vec::new();

    for sym in symbols {
        let sym_name = sym.name().wrap_err("symbol name has invalid UTF-8")?;
        symbol_sizes.push((sym_name, sym.size()));
    }

    symbol_sizes.sort_by_key(|&(_, size)| size);
    symbol_sizes.reverse();
    Ok(symbol_sizes)
}
