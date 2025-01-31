use std::{
    cmp::Ordering,
    collections::HashMap,
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{anyhow, Result};
use clap::{ArgAction, Parser, Subcommand};
use regex::Regex;
use rusqlite::Connection;
use rustc_serialize::hex::FromHex;
use walkdir::WalkDir;

use utils::{
    db::{insert_term_word, retrieve_records, retrieve_separators, retrieve_to_sep_words},
    decrypted, delete_records, delete_term_word, delete_to_sep_word, encrypted, hashed_name,
    insert_record, insert_to_sep_word, open_db, retrieve_term_words, s_compare,
};

pub mod utils;

#[derive(Debug, Parser, Clone)]
#[command(author,about="File and Directory Names",long_about=None)]
pub struct Args {
    ///file path
    #[arg(required = false)]
    pub files: Option<Vec<String>>,

    ///file path
    #[arg(short = 'f', long, default_value = ".")]
    pub file_path: String,

    ///in place
    #[arg(short = 'i', long, default_value = "false")]
    in_place: bool,

    ///max depth
    #[arg(short = 'd', long, default_value = "1")]
    pub max_depth: usize,

    ///file type,'f' for regular file and 'd' for directory
    #[arg(short = 't', long, default_value = "f")]
    pub filetype: String,

    ///not ignore hidden file
    #[arg(short = 'I', long, default_value = "false")]
    not_ignore_hidden: bool,

    ///exclude file or directory
    #[arg(short = 'X', long, default_values_t = Vec::<String>::new(), action = ArgAction::Append)]
    pub exclude_path: Vec<String>,

    ///reverse change
    #[arg(short = 'r', long, default_value = "false")]
    pub reverse: bool,

    ///reverse change chainly
    #[arg(short = 'R', long, default_value = "false")]
    pub reverse_chainly: bool,

    ///align origin and edited
    #[arg(short = 'a', long, default_value = "false")]
    align: bool,

    ///print version
    #[arg(short = 'V', long)]
    pub version: bool,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Debug, Subcommand, Clone)]
pub enum Commands {
    ///Config pattern
    Config {
        ///List all configurations
        #[arg(short = 'l', long, default_value = "false")]
        list: bool,

        ///Config Separators,Terms ...
        #[arg(short = 'c', long)]
        add: Option<String>,

        ///Delete configurations
        #[arg(short = 'd', long)]
        delete: Option<String>,
    },

    ///Change file name directly
    Mv {
        ///Input source file path and target file name
        #[clap(required = true)]
        inputs: Vec<String>,
    },
}

#[derive(Debug, Clone)]
pub struct DirBase {
    pub dir: String,
    pub base: String,
}

#[derive(Clone)]
pub struct Separator {
    id: i32,
    pub value: String,
}

impl Default for Separator {
    fn default() -> Self {
        Self {
            id: 0,
            value: "_".to_owned(),
        }
    }
}
pub struct ToSepWord {
    id: i32,
    pub value: String,
}

pub struct TermWord {
    id: i32,
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone)]
pub struct Record {
    id: i32,
    hashed_current_name: String,
    encrypted_pre_name: String,
    count: i32,
}

impl Record {
    pub fn new(origin: &str, target: &str) -> Result<Self> {
        let hashed = hashed_name(target);
        let encrypted = encrypted(origin, target)?;
        Ok(Self {
            id: 0,
            hashed_current_name: hashed,
            encrypted_pre_name: encrypted,
            count: 1,
        })
    }
}

///Return absolute paths
pub fn regular_files(directory: &Path, depth: usize, excludes: Vec<&Path>) -> Result<Vec<PathBuf>> {
    let mut paths: Vec<_> = WalkDir::new(directory)
        .max_depth(depth)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter_map(|entry| {
            if entry.file_type().is_file() {
                Some(entry.into_path())
            } else {
                None
            }
        })
        .collect();

    paths.retain(|path| !excludes.iter().any(|exc| path.starts_with(exc)));

    Ok(paths)
}

///Return directories
pub fn directories(directory: &Path, depth: usize, excludes: Vec<&Path>) -> Result<Vec<PathBuf>> {
    let mut paths: Vec<_> = WalkDir::new(directory)
        .max_depth(depth)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter_map(|entry| {
            if entry.file_type().is_dir() {
                Some(entry.into_path())
            } else {
                None
            }
        })
        .collect();

    paths.retain(|path| !excludes.iter().any(|exc| path.starts_with(exc)));

    paths.sort_by(|a, b| {
        let a_str = a.as_os_str();
        let b_str = b.as_os_str();
        match a_str.cmp(b_str) {
            Ordering::Equal => a_str.len().cmp(&b_str.len()),
            other => other.reverse(),
        }
    });

    Ok(paths)
}

///Create DirBase struct from abs_path
fn dir_base(abs_path: &Path) -> Option<DirBase> {
    if let (Some(base), Some(dir_path)) = (abs_path.file_name(), abs_path.parent()) {
        if let (Some(dir), Some(base)) = (dir_path.to_str(), base.to_str()) {
            Some(DirBase {
                dir: dir.to_owned(),
                base: base.to_owned(),
            })
        } else {
            None
        }
    } else {
        None
    }
}

///Check a unix path is hidden or not
#[cfg(unix)]
fn is_hidden_unix(path: &Path) -> bool {
    match path.file_name() {
        Some(n) => match n.to_str() {
            Some(s) => s.starts_with('.'),
            None => false,
        },
        None => false,
    }
}

#[cfg(windows)]
use std::os::windows::fs::MetadataExt;
#[cfg(windows)]
use winapi::um::winnt::FILE_ATTRIBUTE_HIDDEN;

///Check a windows path is hidden or not
#[cfg(windows)]
fn is_hidden_windows(path: &Path) -> bool {
    match path.metadata() {
        Ok(metadata) => metadata.file_attributes() & FILE_ATTRIBUTE_HIDDEN != 0,
        Err(_) => false, // If there's an error, assume it's not hidden
    }
}

///Check a path is hidden or not
fn is_hidden(path: &Path) -> bool {
    #[cfg(unix)]
    {
        is_hidden_unix(path)
    }
    #[cfg(windows)]
    {
        is_hidden_windows(path)
    }
}

///Remove continuouse "word" in "source"
fn remove_continuous(source: &str, word: &str) -> Result<String> {
    let re = Regex::new(&format!(r"(?i){}{}+", word, word))?;
    Ok(re.replace_all(source, word).to_string())
}

///Remove prefix separator and suffix separator
fn remove_prefix_sep_suffix_sep<'a>(s: &'a str, sep: &'a str) -> &'a str {
    let s = s.strip_prefix(sep).unwrap_or(s);
    s.strip_suffix(&sep).unwrap_or(s)
}

///Rename a file or directory's name into specific target or by default
fn fdn_f(dir_base: &DirBase, target: Option<String>, in_place: bool) -> Result<String> {
    let conn = open_db(None)?;

    let sep = retrieve_separators(&conn)?;
    let sep = {
        if !sep.is_empty() {
            sep[0].clone().value
        } else {
            Separator::default().value
        }
    };

    let mut base_name = dir_base.base.to_owned();

    let s_path = Path::new(&dir_base.dir).join(dir_base.base.clone());

    let t_path = match target {
        Some(tn) => {
            base_name.clone_from(&tn);
            Path::new(&dir_base.dir).join(tn)
        }
        None => {
            let (f_stem, f_ext) = match s_path.is_file() {
                true => (
                    Path::new(&base_name).file_stem(),
                    Path::new(&base_name).extension().and_then(OsStr::to_str),
                ),
                false => (Some(Path::new(&base_name).as_os_str()), None),
            };

            let mut f_stem = os2string(f_stem)?;

            //replace to sep words
            let to_sep_words = retrieve_to_sep_words(&conn)?;
            let replacements_map: HashMap<_, _> = to_sep_words
                .iter()
                .map(|e| (e.value.clone(), sep.clone()))
                .collect();

            let mut old_f_stem = f_stem.clone();
            loop {
                replacements_map.iter().for_each(|(k, v)| {
                    f_stem = f_stem.replace(k, v);
                });
                if old_f_stem.eq(&f_stem) {
                    break;
                }
                old_f_stem.clone_from(&f_stem);
            }

            //term words
            let term_words = retrieve_term_words(&conn)?;
            let replacements_map: HashMap<_, _> = term_words
                .iter()
                .map(|e| (e.key.clone(), e.value.clone()))
                .collect();
            let mut old_f_stem = f_stem.clone();
            loop {
                replacements_map.iter().for_each(|(k, v)| {
                    f_stem = f_stem.replace(k, v);
                });
                if old_f_stem.eq(&f_stem) {
                    break;
                }
                old_f_stem.clone_from(&f_stem);
            }

            //remove continuous
            f_stem = remove_continuous(&f_stem, &sep)?;

            //remove prefix and suffix sep
            let rlt = remove_prefix_sep_suffix_sep(&f_stem, &sep).to_owned();
            f_stem = rlt;

            base_name = match f_ext {
                Some(f_ext) => format!("{}.{}", f_stem, f_ext),
                None => f_stem.to_owned(),
            };
            Path::new(&dir_base.dir).join(base_name.clone())
        }
    };

    //take effect
    if base_name != dir_base.base && in_place {
        fs::rename(s_path, t_path)?;
        let rd = Record::new(&dir_base.clone().base, &base_name)?;
        insert_record(&conn, rd)?;
    }

    Ok(base_name)
}

///Firstly rename files or directories's name into targets or by default,then do post-processing work
pub fn fdn_fs_post(origins: Vec<PathBuf>, targets: Vec<String>, args: Args) -> Result<()> {
    let mut tgts: Vec<Option<String>> = vec![None];

    if targets.is_empty() {
        tgts.resize(origins.len(), None);
    } else if origins.len() != targets.len() {
        return Err(anyhow!(
            "origins length {:?} must equal to targets length {:?}",
            origins.len(),
            targets.len()
        ));
    } else {
        tgts = targets.into_iter().map(Some).collect();
    }

    origins
        .iter()
        .zip(tgts.iter())
        .filter(|(of, _tn)| !(is_hidden(of) && args.not_ignore_hidden))
        .try_for_each(|(of, tn)| -> Result<()> {
            if let Some(d_b) = dir_base(of) {
                let rlt = fdn_f(&d_b, tn.clone(), args.in_place)?;

                let (o_r, e_r) = match args.align {
                    true => fname_compare(&d_b.base, &rlt, "a")?,
                    false => fname_compare(&d_b.base, &rlt, "")?,
                };
                if !o_r.eq(&e_r) {
                    if args.in_place {
                        println!("   {}\n==>{}", o_r, e_r);
                    } else {
                        println!("   {}\n-->{}", o_r, e_r);
                    }
                }
            }
            Ok(())
        })?;

    Ok(())
}

///Revertly rename a file or directory's name
fn fdn_rf(dir_base: &DirBase, in_place: bool) -> Result<Option<String>> {
    let conn = open_db(None)?;

    let base_name = &dir_base.base;
    let rds = retrieve_records(&conn)?;

    let map: HashMap<_, _> = rds
        .iter()
        .map(|rd| (rd.clone().hashed_current_name, rd.clone()))
        .collect();
    let rd = map.get(&hashed_name(base_name));

    match rd {
        Some(rd) => match decrypted(&rd.encrypted_pre_name, base_name) {
            Ok(v) => {
                let rt = v.from_hex()?;
                let base_name = String::from_utf8(rt)?;
                //take effect
                if in_place {
                    let s_path = Path::new(&dir_base.dir).join(dir_base.base.clone());
                    let t_path = Path::new(&dir_base.dir).join(base_name.clone());
                    fs::rename(s_path, t_path)?; //Only rename successfully then ...
                    if rd.count == 1 {
                        delete_records(&conn, rd.id)?;
                    }
                }
                Ok(Some(base_name))
            }
            Err(err) => Err(err),
        },
        None => Ok(None),
    }
}

///Firstly revertly rename files or directories's name,then do post-processing work
pub fn fdn_rfs_post(files: Vec<PathBuf>, args: Args) -> Result<()> {
    files
        .iter()
        .filter(|f| args.not_ignore_hidden || !is_hidden(f))
        .try_for_each(|f| -> Result<()> {
            let mut frc = Some(f.clone());
            while let Some(ref f) = frc {
                if let Some(dir_base) = dir_base(f) {
                    match fdn_rf(&dir_base, args.in_place) {
                        Ok(Some(rf_base)) => {
                            if args.reverse_chainly {
                                frc = Some(Path::new(&dir_base.dir).join(rf_base.clone()));
                            } else {
                                frc = None;
                            }
                            let (o_r, e_r) = match args.align {
                                true => fname_compare(&dir_base.base, &rf_base, "a")?,
                                false => fname_compare(&dir_base.base, &rf_base, "")?,
                            };
                            if !o_r.eq(&e_r) {
                                if args.in_place {
                                    println!("   {}\n==>{}", o_r, e_r);
                                } else {
                                    println!("   {}\n-->{}", o_r, e_r);
                                }
                            }
                        }
                        Ok(None) => break,
                        Err(err) => return Err(err),
                    }
                }
            }

            Ok(())
        })?;

    Ok(())
}

fn os2string(input: Option<&OsStr>) -> Result<String> {
    match input {
        Some(os_str) => match os_str.to_str() {
            Some(valid_str) => Ok(valid_str.to_string()),
            None => Err(anyhow!("Invalid UTF-8 sequence")),
        },
        None => Err(anyhow!("Option is None")),
    }
}

///return unicode names of every character of the string and name separated by "," sign
fn unames(s: &str) -> String {
    s.chars()
        .filter_map(|c| unicode_names2::name(c).map(|n| n.to_string()))
        .collect::<Vec<_>>()
        .join(",")
}

///list all separators stored in database via database connection
fn list_separators(conn: &Connection) -> Result<()> {
    let mut rlts = retrieve_separators(conn)?;
    let s = "Separator";
    println!("{} ID\tValue\tDescription", s);
    rlts.sort_by_key(|sep| sep.id);
    rlts.iter().for_each(|sep| {
        println!(
            "{} {}\t{}\t{}",
            " ".repeat(s.len()),
            sep.id,
            sep.value,
            unames(&sep.value)
        );
    });

    Ok(())
}

///list all to separator words stored in database via database connection
fn list_to_sep_words(conn: &Connection) -> Result<()> {
    let mut rlts = retrieve_to_sep_words(conn)?;
    let s = "ToSepWord";
    println!("{} ID\tValue\tDescription", s);
    rlts.sort_by_key(|tsw| tsw.id);
    rlts.iter().for_each(|tsw| {
        println!(
            "{} {}\t{}\t{}",
            " ".repeat(s.len()),
            tsw.id,
            tsw.value.replace('\r', "\\r").replace('\n', "\\n"),
            unames(&tsw.value)
        );
    });

    Ok(())
}

///list all term words stored in database via database connection
fn list_term_words(conn: &Connection) -> Result<()> {
    let mut rlts = retrieve_term_words(conn)?;
    let s = "TermWord";
    println!("{} ID\tKey\tValue", s);
    rlts.sort_by_key(|tw| tw.id);
    rlts.iter().for_each(|tw| {
        println!(
            "{} {}\t{}\t{}",
            " ".repeat(s.len()),
            tw.id,
            tw.key,
            tw.value.replace('\r', "\\r").replace('\n', "\\n")
        );
    });

    Ok(())
}

///List all configurations
pub fn config_list() -> Result<()> {
    let conn = open_db(None)?;
    list_separators(&conn)?;
    list_to_sep_words(&conn)?;
    list_term_words(&conn)?;

    Ok(())
}

///Add configuration into database
pub fn config_add(word: &str) -> Result<()> {
    let conn = open_db(None)?;
    match word.split_once(':') {
        Some((key, value)) => {
            insert_term_word(&conn, key, value)?;
            list_term_words(&conn)?;
        }
        None => {
            insert_to_sep_word(&conn, word)?;
            list_to_sep_words(&conn)?;
        }
    }

    Ok(())
}

///Delete configuration in the database
pub fn config_delete(word: &str) -> Result<()> {
    let conn = open_db(None)?;
    match word.split_once(':') {
        Some((key, value)) => {
            let rlts = retrieve_term_words(&conn)?;
            let the_word = rlts.iter().find(|&w| w.key == key && w.value == value);
            if let Some(w) = the_word {
                delete_term_word(&conn, w.id)?;
                list_term_words(&conn)?;
            }
        }
        None => {
            let rlts = retrieve_to_sep_words(&conn)?;
            let the_word = rlts.iter().find(|&w| w.value == word);
            if let Some(w) = the_word {
                delete_to_sep_word(&conn, w.id)?;
                list_to_sep_words(&conn)?;
            }
        }
    }

    Ok(())
}

///compare file stem and file extension separately and return rich text
fn fname_compare(origin: &str, edit: &str, mode: &str) -> Result<(String, String)> {
    let (o_stem, o_ext) = stem_ext(origin)?;
    let (e_stem, e_ext) = stem_ext(edit)?;

    let (o_stem_cmp, e_stem_cmp) = s_compare(&o_stem, &e_stem, mode)?;
    let (o_ext_cmp, e_ext_cmp) = s_compare(&o_ext, &e_ext, mode)?;

    Ok((
        o_stem_cmp + if o_ext.is_empty() { "" } else { "." } + &o_ext_cmp,
        e_stem_cmp + if e_ext.is_empty() { "" } else { "." } + &e_ext_cmp,
    ))
}

///return file stem and file extension by file path
fn stem_ext<P>(path: P) -> Result<(String, String)>
where
    P: AsRef<Path> + AsRef<OsStr>,
{
    let p = Path::new(&path);
    let stem = os2string(p.file_stem())?;
    let ext = os2string(p.extension())?;

    Ok((stem, ext))
}

#[cfg(test)]
mod tests {
    use crate::{remove_continuous, remove_prefix_sep_suffix_sep, stem_ext};

    #[test]
    fn test_remove_xfix_sep() {
        let sep = "_";
        let s = "_PDFScholar_";
        assert!(s.starts_with(sep));
        assert!(s.ends_with(sep));
        let t = "PDFScholar";
        assert_eq!(remove_prefix_sep_suffix_sep(s, sep), t);
    }

    #[test]
    fn test_stem_ext() {
        let p = "stem.ext";
        let (s, e) = stem_ext(p).unwrap();
        assert!(s.eq("stem"));
        assert!(e.eq("ext"));
    }

    #[test]
    fn test_remove_continuous() {
        let src = "A_B__C___D_.txt";
        let sep = "_";
        let tgt = "A_B_C_D_.txt";
        assert_eq!(remove_continuous(src, sep).unwrap(), tgt);
    }
}
