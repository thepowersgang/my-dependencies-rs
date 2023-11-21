//! Provides the `enumerate` function, that lists all of the dependencies to the current crate.
//!
//! This is designed to be run as part of a build script, so don't expect much luck without it.

use std::collections::{HashMap, HashSet};

#[derive(Debug)]
/// A cargo depdencency
pub struct ActiveDependency
{
    /// Source for the dependency code
    pub source: DepSource,
    /// Are default features included
    pub include_default_features: bool,
    /// Set of explicitly enabled features
    pub features: HashSet<String>,
}
#[derive(Debug)]
pub enum DepSource
{
    /// From git
    Git {
        /// repository URL
        url: String,
        /// Which particular revision to use
        revision: GitRev,
        },
    /// Path dependency
    Path(String),
    /// A crates.io depdencency
    CratesIo(String),
	/// The soruce isn't known (due to a file error, or incomplete workspace information)
	Unknown,
}
#[derive(Debug)]
pub enum GitRev
{
    /// No specified source, fetches from HEAD of mastr
    Master,
    /// Fetch from HEAD of the given branch
    Branch(String),
    /// Fetch the given tag
    Tag(String),
    /// Fetch a specific revision
    Revision(String),
}

/// Enumerate all dependencies that are currently available to the crate
///
/// This obtains all unconditional dependencies AND all enabled conditional deps (based on features
/// and targets)
pub fn enumerate() -> HashMap<String, ActiveDependency>
{
    let manifest_path = {
        let mut p = std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
        p.push("Cargo.toml");
        p
        };
    let content = match std::fs::read(&manifest_path)
        {
        Ok(v) => v,
        Err(e) => panic!("Unable to open {}: {:?}", manifest_path.display(), e),
        };
    let m = match cargo_toml::Manifest::from_slice(&content)
        {
        Ok(v) => v,
        Err(e) => panic!("Unable to parse {}: {:?}", manifest_path.display(), e),
        };
    
    // Enumerate which of the declared features are active (activates dependency features)
    let mut dep_features = HashMap::<String, HashSet<String>>::new();
    for (feat_name, subfeats) in m.features
    {
        if std::env::var_os(format!("CARGO_FEATURE_{}", feat_name)).is_some()
        {
            for subfeat_desc in subfeats
            {
                let (dep, f) = {
                    let mut it = subfeat_desc.split('/');
                    ( it.next().unwrap(), it.next().unwrap(), )
                    };
                dep_features.entry(dep.to_string()).or_default().insert(f.to_string());
            }
        }
    }
    
    let mut rv = HashMap::new();
    for (depname, dep_info) in m.dependencies
    {
        if let Some(ad) = get_activedep(&dep_features, &depname, &dep_info)
        {
            rv.insert(depname.clone(), ad);
        }
    }
    
    let current_target = std::env::var("TARGET").unwrap();
    for (target_name, target_info) in m.target
    {
        // If the target begins with 'cfg', parse it as a cfg fragment
        let active =
            if target_name.starts_with("cfg") {
                let ml: syn::MetaList = match syn::parse_str(&target_name)
                    {
                    Ok(v) => v,
                    Err(e) => panic!("Failed to parse target cfg {:?} - {:?}", target_name, e),
                    };
                match check_cfg_root(&ml)
                {
                Some(v) => v,
                None => panic!("Failed to parse target cfg - {:?}", target_name),
                }
            }
            else {
                target_name == current_target
            };
        // If this target applies, enumerate dependencies
        if active
        {
            for (depname, dep_info) in target_info.dependencies
            {
                if let Some(ad) = get_activedep(&dep_features, &depname, &dep_info)
                {
                    rv.insert(depname.clone(), ad);
                }
            }
        }
    }
    
    rv
}

/// Get an "ActiveDependency" for this `cargo_toml` dependency
fn get_activedep(dep_features: &HashMap<String, HashSet<String>>, depname: &str, dep_info: &cargo_toml::Dependency) -> Option<ActiveDependency>
{
    Some(match dep_info
    {
    cargo_toml::Dependency::Simple(version_str) => {
        ActiveDependency {
            source: DepSource::CratesIo(version_str.clone()),
            include_default_features: true,
            features: dep_features.get(depname).cloned().unwrap_or(HashSet::new()),
            }
        },
    cargo_toml::Dependency::Inherited(details) => {
        if details.optional && std::env::var_os(format!("CARGO_FEATURE_{}", depname)).is_none() {
            return None;
        }
		// Cannot get the full source without workspace info
        let source = DepSource::Unknown;
        let mut features = dep_features.get(depname).cloned().unwrap_or(HashSet::new());
        for f in &details.features
        {
            features.insert(f.clone());
        }
        ActiveDependency {
            source: source,
            include_default_features: false,	// This depends on if the workspace asked for default features
            features: features,
            }
		},
    cargo_toml::Dependency::Detailed(details) => {
        if details.optional && std::env::var_os(format!("CARGO_FEATURE_{}", depname)).is_none() {
            return None;
        }
        let source = 
            if let Some(ref version_str) = details.version {
                DepSource::CratesIo(version_str.clone())
            }
            else if let Some(ref path) = details.path {
                DepSource::Path(path.clone())
            }
            else if let Some(ref url) = details.git {
                DepSource::Git {
                    url: url.clone(),
                    revision: if let Some(ref rev) = details.rev {
                            GitRev::Revision(rev.clone())
                        }
                        else if let Some(ref tag) = details.tag {
                            GitRev::Tag(tag.clone())
                        }
                        else if let Some(ref branch) = details.branch {
                            GitRev::Branch(branch.clone())
                        }
                        else {
                            GitRev::Master
                        },
                    }
            }
            else {
                DepSource::Unknown
            };
        let mut features = dep_features.get(depname).cloned().unwrap_or(HashSet::new());
        for f in &details.features
        {
            features.insert(f.clone());
        }
        ActiveDependency {
            source: source,
            include_default_features: details.default_features,
            features: features,
            }
        },
    })
}

/// Check `cfg()`-style targets
fn check_cfg_root(ml: &syn::MetaList) -> Option<bool>
{
    if ml.nested.len() != 1 {
        eprintln!("Unexpected cfg(...) takes a single argument, {} provided", ml.nested.len());
        return None;
    }
    check_cfg( ml.nested.first().unwrap() )
}
fn check_cfg(m: &syn::NestedMeta) -> Option<bool>
{
    let m = match m
        {
        syn::NestedMeta::Meta(m) => m,
        _ => return None,
        };
    Some(match m
    {
    syn::Meta::Path(_) => return None,
    syn::Meta::List(ml) => {
        let i = ml.path.get_ident()?;
        if i == "any" {
            for e in &ml.nested {
                if check_cfg(e)? {
                    return Some(true);
                }
            }
            false
        }
        else if i == "not" {
            if ml.nested.len() != 1 {
                eprintln!("Unexpected not(...) takes a single argument, {} provided", ml.nested.len());
                return None;
            }
            let e = ml.nested.first().unwrap();
            ! check_cfg(e)?
        }
        else if i == "all" {
            for e in &ml.nested {
                if ! check_cfg(e)? {
                    return Some(false);
                }
            }
            true
        }
        else {
            eprintln!("Unexpected cfg fragment: {}", i);
            return None;
        }
        },
    syn::Meta::NameValue(nv) => {
        let i = nv.path.get_ident()?;
        let v = match &nv.lit
            {
            syn::Lit::Str(s) => s.value(),
            _ => {
                eprintln!("cfg options require strings, got {:?}", nv.lit);
                return None;
                },
            };
        let ev = std::env::var(format!("CARGO_CFG_{}", i));
        ev == Ok(v)
        },
    })
}
