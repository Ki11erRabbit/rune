use crate::core::arena::Rt;
use crate::core::arena::{Arena, Root};
use crate::core::env::Symbol;
use crate::core::env::{sym, Environment};
use crate::core::error::{Error, Type};
use crate::core::object::{GcObj, Object};
use crate::reader;
use crate::{interpreter, root};
use fn_macros::defun;

use anyhow::{bail, ensure, Context, Result};

use std::fs;
use std::path::{Path, PathBuf};

fn check_lower_bounds(idx: Option<i64>, len: usize) -> Result<usize> {
    let len = len as i64;
    let idx = idx.unwrap_or(0);
    ensure!(
        -len <= idx && idx < len,
        "start index of {idx} is out of bounds for string of length {len}"
    );
    let idx = if idx < 0 { len + idx } else { idx };
    Ok(idx as usize)
}

fn check_upper_bounds(idx: Option<i64>, len: usize) -> Result<usize> {
    let len = len as i64;
    let idx = idx.unwrap_or(len);
    ensure!(
        -len <= idx && idx <= len,
        "end index of {idx} is out of bounds for string of length {len}"
    );
    let idx = if idx < 0 { len + idx } else { idx };
    Ok(idx as usize)
}

#[defun]
pub(crate) fn read_from_string<'ob>(
    string: &str,
    start: Option<i64>,
    end: Option<i64>,
    arena: &'ob Arena,
) -> Result<GcObj<'ob>> {
    let len = string.len();
    let start = check_lower_bounds(start, len)?;
    let end = check_upper_bounds(end, len)?;

    let (obj, new_pos) = match reader::read(&string[start..end], arena) {
        Ok((obj, pos)) => (obj, pos),
        Err(mut e) => {
            e.update_pos(start);
            bail!(e);
        }
    };
    Ok(cons!(obj, new_pos as i64; arena))
}

pub(crate) fn load_internal<'ob>(
    contents: &str,
    arena: &'ob mut Arena,
    env: &mut Root<Environment>,
) -> Result<bool> {
    let mut pos = 0;
    loop {
        let (obj, new_pos) = match reader::read(&contents[pos..], arena) {
            Ok((obj, pos)) => (obj, pos),
            Err(reader::Error::EmptyStream) => return Ok(true),
            Err(mut e) => {
                e.update_pos(pos);
                bail!(e);
            }
        };
        if crate::debug::debug_enabled() {
            let content = &contents[pos..(new_pos + pos)];
            println!("-----READ START-----\n {content}");
            println!("-----READ END-----");
        }
        root!(obj, arena);
        interpreter::eval(obj, None, env, arena)?;
        assert_ne!(new_pos, 0);
        pos += new_pos;
    }
}

fn file_in_path(file: &str, path: &str) -> Option<PathBuf> {
    let path = Path::new(path).join(file);
    if Path::new(&path).exists() {
        Some(path)
    } else {
        let with_ext = path.with_extension("el");
        Path::new(&with_ext).exists().then(|| with_ext)
    }
}

fn find_file_in_load_path(
    file: &str,
    arena: &Arena,
    env: &mut Root<Environment>,
) -> Result<PathBuf> {
    let load_path = env.deref_mut(arena).vars.get(&sym::LOAD_PATH).unwrap();
    let paths = load_path
        .bind(arena)
        .as_list()
        .context("`load-path' was not a list")?;
    let mut final_file = None;
    for path in paths {
        match path?.get() {
            Object::String(path) => {
                if let Some(x) = file_in_path(file, path) {
                    final_file = Some(x);
                    break;
                }
            }
            x => {
                return Err(Error::from_object(Type::String, x))
                    .context("Found non-string in `load-path'")
            }
        }
    }
    match final_file {
        Some(x) => Ok(x),
        None => bail!("Unable to find file {file} in load-path"),
    }
}

#[defun]
pub(crate) fn load<'ob>(
    file: &Rt<GcObj>,
    noerror: Option<()>,
    nomessage: Option<()>,
    arena: &'ob mut Arena,
    env: &mut Root<Environment>,
) -> Result<bool> {
    let file = match file.bind(arena).get() {
        Object::String(x) => x,
        x => bail!(Error::from_object(Type::Symbol, x)),
    };
    let final_file = if Path::new(file).exists() {
        PathBuf::from(file)
    } else {
        find_file_in_load_path(file, arena, env)?
    };

    if nomessage.is_none() {
        println!("Loading {file}...");
    }
    match fs::read_to_string(&final_file)
        .with_context(|| format!("Couldn't open file {:?}", final_file.as_os_str()))
    {
        Ok(content) => load_internal(&content, arena, env),
        Err(e) => match noerror {
            Some(()) => Ok(false),
            None => Err(e),
        },
    }
}

#[defun]
pub(crate) fn intern(string: &str) -> Symbol {
    crate::core::env::intern(string)
}

defsubr!(load, read_from_string, intern);

#[cfg(test)]
mod test {

    use super::*;
    use crate::core::arena::RootSet;
    use crate::root;

    #[test]
    #[allow(clippy::float_cmp)] // Bug in Clippy
    fn test_load() {
        let roots = &RootSet::default();
        let arena = &mut Arena::new(roots);
        root!(env, Environment::default(), arena);
        load_internal("(setq foo 1) (setq bar 2) (setq baz 1.5)", arena, env).unwrap();

        let obj = reader::read("(+ foo bar baz)", arena).unwrap().0;
        root!(obj, arena);
        let val = interpreter::eval(obj, None, env, arena).unwrap();
        assert_eq!(val, 4.5);
    }
}
