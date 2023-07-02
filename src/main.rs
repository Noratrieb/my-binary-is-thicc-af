use std::collections::HashMap;

use eyre::{eyre, Context, ContextCompat, Result};
use object::{Object, ObjectSection, ObjectSymbol};
use serde::Serialize;

#[derive(serde::Serialize)]
struct SerGroup {
    id: String,
    label: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    groups: Vec<SerGroup>,
}

fn main() -> Result<()> {
    let path = std::env::args()
        .nth(1)
        .unwrap_or("./target/debug/my-binary-is-thicc-af".into());

    let data = std::fs::read(&path).wrap_err_with(|| format!("error opening `{path}`"))?;
    let object = object::File::parse(data.as_slice()).context("could not parse object file")?;

    let text = object
        .section_by_name(".text")
        .ok_or_else(|| eyre!("could not find .text section"))?;

    let symbols = object.symbols();

    let text_range = text.address()..(text.address() + text.size());

    let mut symbols_sorted = symbols
        .into_iter()
        .filter(|sym| text_range.contains(&sym.address()))
        .collect::<Vec<_>>();

    symbols_sorted.sort_by_key(|s| s.address());

    let mut symbol_sizes = Vec::new();

    for syms in symbols_sorted.windows(2) {
        let [first, second] = syms else {
            unreachable!()
        };
        let first_size = second.address() - first.address();

        let sym_name = first.name().wrap_err("symbol name has invalid UTF-8")?;

        symbol_sizes.push((sym_name, first_size));
    }

    symbol_sizes.sort_by_key(|&(_, size)| size);
    symbol_sizes.reverse();

    let mut root_groups = Groups(HashMap::new());

    for (sym, size) in symbol_sizes {
        let components = symbol_components(sym).with_context(|| sym.to_string())?;

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
struct Groups(HashMap<String, Group>);

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
            children: Groups(HashMap::new()),
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

fn symbol_components(sym: &str) -> Result<Vec<String>> {
    let demangled = rustc_demangle::demangle(sym).to_string();

    let components = if demangled.starts_with('<') {
        parse_qpath(&demangled)
            .context("invalid qpath")
            .and_then(|qpath| qpath_components(qpath))
            .unwrap_or_else(|_| demangled.split("::").collect::<Vec<_>>())
    } else {
        // normal path
        demangled.split("::").collect::<Vec<_>>()
    };

    let components = components
        .into_iter()
        .map(|c| {
            if c.contains(",") {
                format!("\"{c}\"")
            } else {
                c.to_owned()
            }
        })
        .collect::<Vec<_>>();

    // qpath
    return Ok(components);
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct QPath<'a> {
    qself: &'a str,
    trait_: &'a str,
    pathy_bit: &'a str,
}

fn qpath_components(qpath: QPath<'_>) -> Result<Vec<&str>> {
    if qpath.qself.starts_with('<') {
        if let Ok(sub_qpath) = parse_qpath(qpath.qself) {
            let mut sub_components = qpath_components(sub_qpath)?;
            sub_components.extend(qpath.pathy_bit.split("::"));
            Ok(sub_components)
        } else {
            Ok(qpath
                .qself
                .split("::")
                .chain(qpath.pathy_bit.split("::"))
                .collect())
        }
    } else {
        Ok(qpath
            .qself
            .split("::")
            .chain(qpath.pathy_bit.split("::"))
            .collect())
    }
}

// FIXME: Apparently the symbol `std::os::linux::process::<impl core::convert::From<std::os::linux::process::PidFd> for std::os::fd::owned::OwnedFd>::from` exists in std
// I have no clue what to do about that.

fn parse_qpath(s: &str) -> Result<QPath<'_>> {
    let mut chars = s.char_indices().skip(1);
    let mut angle_brackets = 1u64;

    let mut result = None;
    let mut as_idx = None;

    while let Some((idx, char)) = chars.next() {
        match char {
            '<' => angle_brackets += 1,
            '>' => {
                angle_brackets -= 1;
                if angle_brackets == 0 {
                    result = Some(idx);
                    break;
                }
            }
            ' ' => {
                if angle_brackets == 1 && as_idx == None {
                    as_idx = Some(idx);
                }
            }
            _ => {}
        }
    }

    let q_close_idx = result.wrap_err_with(|| {
        format!("qualified symbol `{s}` does not end qualified part with > properly")
    })?;

    let as_idx =
        as_idx.wrap_err_with(|| format!("qualified symbol `{s}` does not contain ` as `"))?;

    let q = &s[..q_close_idx];
    let pathy_bit = &s[q_close_idx + 1..];
    let pathy_bit = pathy_bit.strip_prefix("::").wrap_err_with(|| {
        format!("path after qualification does not start with `::`: `{pathy_bit}`")
    })?;

    let qself = &q[1..as_idx];
    let trait_ = &q[(as_idx + " as ".len())..];

    Ok(QPath {
        qself,
        trait_,
        pathy_bit,
    })
}

#[cfg(test)]
mod tests {
    use super::QPath;

    use super::parse_qpath;

    #[test]
    fn parse_qpaths() {
        assert_eq!(
            parse_qpath("<std::path::Components as core::fmt::Debug>::fmt").unwrap(),
            QPath {
                qself: "std::path::Components",
                trait_: "core::fmt::Debug",
                pathy_bit: "fmt",
            }
        );

        assert_eq!(
            parse_qpath("<<std::path::Components as core::fmt::Debug>::fmt::DebugHelper as core::fmt::Debug>::fmt").unwrap(),
            QPath {
                qself: "<std::path::Components as core::fmt::Debug>::fmt::DebugHelper",
                trait_: "core::fmt::Debug",
                pathy_bit: "fmt",
            }
        );
    }
}
