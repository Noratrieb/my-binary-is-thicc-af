//! This is really hacky code where we best-effort extract some sort of tree
//! from symbols. It's bad.
#![allow(dead_code)]

use std::{fmt::Debug, iter::Peekable, str::CharIndices};

use eyre::{bail, Context, ContextCompat, Result};

pub fn symbol_components(sym: &str) -> Result<Vec<String>> {
    let demangled = rustc_demangle::demangle(sym).to_string();

    // If the symbol is a qualified path (`<T as Tr>::m`), then we need to parse
    // it as such.
    let components = if demangled.starts_with('<') {
        parse_qpath(&demangled)
            .wrap_err("invalid qpath")
            .and_then(|qpath| qpath_components(qpath))
            .unwrap_or_else(|_| demangled.split("::").collect::<Vec<_>>())
    } else {
        // This is not a
        demangled.split("::").collect::<Vec<_>>()
    };

    let components = components
        .into_iter()
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();

    // qpath
    Ok(components)
}

#[derive(PartialEq)]
pub struct Path<'a>(Vec<PathSegment<'a>>);

#[derive(PartialEq)]
pub struct PathSegment<'a> {
    path: &'a str,
    generic_args: Vec<Path<'a>>,
}

impl Debug for Path<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            self.0
                .iter()
                .map(|s| format!("{s:?}"))
                .collect::<Vec<_>>()
                .join(",")
        )
    }
}

impl Debug for PathSegment<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.generic_args.is_empty() {
            write!(f, "{}", &self.path)
        } else {
            write!(
                f,
                "{}[{}]",
                &self.path,
                self.generic_args
                    .iter()
                    .map(|p| format!("{p:?}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
    }
}

#[derive(PartialEq)]
enum PathFinished {
    Yes,
    No,
}

pub fn parse_path<'a>(path: &'a str, chars: &mut Peekable<CharIndices<'a>>) -> Result<Path<'a>> {
    let mut segments = Vec::new();

    while let Some((idx, c)) = chars.next() {
        match c {
            ':' => {
                if let Some((_, ':')) = chars.peek() {
                    chars.next();
                }
            }
            '<' => {
                unreachable!("path cannot start with <")
            }
            '>' => {
                // generic args closing, we're done.
                return Ok(Path(segments));
            }
            _ => {
                let (segment, finished) = parse_path_segment(path, chars, idx)?;
                dbg!(&segment);

                segments.push(segment);

                if finished != PathFinished::Yes && !matches!(chars.next(), Some((_, ':'))) {
                    bail!("Colon must be followed by second colon");
                }

                // we're done.
                if finished == PathFinished::Yes {
                    return Ok(Path(segments));
                }
            }
        }
    }
    Ok(Path(segments))
}

fn parse_path_segment<'a>(
    path: &'a str,
    chars: &mut Peekable<CharIndices<'a>>,
    start_of_path: usize,
) -> Result<(PathSegment<'a>, PathFinished)> {
    let mut generic_args = Vec::new();

    // TODO: Paths can start with < like <impl i32>. In this case, just treat the entire thing as opaque.

    while let Some((idx, c)) = chars.next() {
        match c {
            ':' | '>' => {
                let component = &path[start_of_path..idx];
                return Ok((
                    PathSegment {
                        path: component,
                        generic_args,
                    },
                    if c == '>' {
                        PathFinished::Yes
                    } else {
                        PathFinished::No
                    },
                ));
            }
            '<' => {
                let arg = parse_path(path, chars)?;
                generic_args.push(arg);
                // > has been eaten by parse_path.
                let component = &path[start_of_path..idx];
                return Ok((
                    PathSegment {
                        path: component,
                        generic_args,
                    },
                    PathFinished::No,
                ));
            }
            _ => {}
        }
    }

    Ok((
        PathSegment {
            path: &path[start_of_path..],
            generic_args,
        },
        PathFinished::Yes,
    ))
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
    use crate::symbol_components;
    use crate::symbols::PathSegment;

    use super::Path;
    use super::QPath;

    use super::parse_qpath;

    fn vec(i: impl IntoIterator<Item = impl Into<String>>) -> Vec<String> {
        i.into_iter().map(Into::into).collect::<Vec<_>>()
    }

    fn parse_path(s: &str) -> Path {
        super::parse_path(s, &mut s.char_indices().peekable()).unwrap()
    }

    #[test]
    fn paths() {
        let seg = |path| PathSegment {
            path,
            generic_args: Vec::new(),
        };
        let seg_gen = |path, generic_args| PathSegment { path, generic_args };
        let single_path = |path| Path(vec![seg(path)]);

        assert_eq!(
            parse_path("core::panicking::panic_nounwind<T>::h078e837899a661cc"),
            Path(vec![
                seg("core"),
                seg("panicking"),
                seg_gen("panic_nounwind", vec![single_path("T")]),
                seg("h078e837899a661cc")
            ])
        );

        assert_eq!(
            parse_path("core::panicking::panic_nounwind::h078e837899a661cc"),
            Path(vec![
                seg("core"),
                seg("panicking"),
                seg("panic_nounwind"),
                seg("h078e837899a661cc")
            ])
        );
    }

    #[test]
    fn components() {
        assert_eq!(
            symbol_components("core::panicking::panic_nounwind::h078e837899a661cc").unwrap(),
            vec(["core", "panicking", "panic_nounwind", "h078e837899a661cc"])
        );
        assert_eq!(
            symbol_components("std::sync::once_lock::OnceLock<T>::initialize::h37ee4f85094ef3f6")
                .unwrap(),
            vec([
                "std",
                "sync",
                "once_lock",
                "OnceLock<T>",
                "initialize",
                "h37ee4f85094ef3f6"
            ])
        );
        assert_eq!(
            symbol_components("<&T as core::fmt::Debug>::fmt::h59637bc6facdc591").unwrap(),
            vec(["&T", "fmt", "h59637bc6facdc591"])
        );
        assert_eq!(
            symbol_components(
                "core::ptr::drop_in_place<gimli::read::abbrev::Attributes>::h180b14c72fab0876"
            )
            .unwrap(),
            vec(["core", "ptr", "drop_in_place"])
        );
    }

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
