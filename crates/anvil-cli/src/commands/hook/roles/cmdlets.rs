//! Path operands of the PowerShell cmdlets that write files, without the values of other parameters.

/// Parameters that name a path.
const PATH_PARAMS: &[&str] = &["path", "literalpath", "destination", "filepath"];
const DESTINATION_ONLY: &[&str] = &["destination"];
/// Parameters that take no value; any other named parameter consumes the next word.
const SWITCHES: &[&str] = &[
    "force",
    "recurse",
    "whatif",
    "confirm",
    "passthru",
    "append",
    "noclobber",
    "nonewline",
    "usetransaction",
    "verbose",
    "debug",
];

/// Which words of a cmdlet call are paths it writes.
struct Cmdlet {
    /// Named parameters that hold a path.
    named: &'static [&'static str],
    /// Positional slots that hold a path.
    positional: &'static [usize],
}

fn cmdlet(name: &str) -> Option<Cmdlet> {
    let (named, positional): (&[&str], &[usize]) = match name {
        "new-item" | "ni" | "set-content" | "add-content" | "ac" | "out-file" | "clear-content"
        | "clc" | "remove-item" | "ri" | "set-location" | "sl" => (PATH_PARAMS, &[0]),
        "move-item" | "mi" | "rename-item" | "rni" | "ren" => (PATH_PARAMS, &[0, 1]),
        "copy-item" | "cpi" => (DESTINATION_ONLY, &[1]),
        _ => return None,
    };
    Some(Cmdlet { named, positional })
}

/// The paths a call to the cmdlet `name` writes. `Copy-Item -Path` only reads, so it counts for
/// nothing; `-NewName` counts only when it is not a bare file name.
pub fn powershell_writes(name: &str, args: &[String]) -> Vec<String> {
    let Some(spec) = cmdlet(name) else {
        return Vec::new();
    };
    let mut paths = Vec::new();
    let mut slot = 0usize;
    let mut words = args.iter();
    while let Some(word) = words.next() {
        let Some(param) = word.strip_prefix('-').filter(|p| p.starts_with(is_name_start)) else {
            if spec.positional.contains(&slot) {
                paths.extend(split_list(word));
            }
            slot += 1;
            continue;
        };
        let (key, inline) = param.split_once(':').map_or((param, None), |(k, v)| (k, Some(v)));
        let key = key.to_ascii_lowercase();
        let value = match inline {
            Some(value) => Some(value.to_owned()),
            None if is_switch(&key) => None,
            None => words.next().cloned(),
        };
        let Some(value) = value else {
            continue;
        };
        if names(spec.named, &key) {
            paths.extend(split_list(&value));
        } else if names(&["newname"], &key) && value.contains(['/', '\\']) {
            paths.push(value);
        }
    }
    paths
}

fn is_name_start(ch: char) -> bool {
    ch.is_ascii_alphabetic()
}

/// PowerShell accepts any unambiguous prefix of a parameter name; three letters keep `-Pa` and
/// `-Pas` (`-PassThru`) apart from `-Path`.
fn names(candidates: &[&str], key: &str) -> bool {
    candidates.iter().any(|full| *full == key || (key.len() >= 3 && full.starts_with(key)))
}

fn is_switch(key: &str) -> bool {
    SWITCHES.iter().any(|full| full.starts_with(key))
}

fn split_list(value: &str) -> impl Iterator<Item = String> + '_ {
    value.split(',').map(str::trim).filter(|v| !v.is_empty()).map(str::to_owned)
}
