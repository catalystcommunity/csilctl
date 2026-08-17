use std::sync::OnceLock;

const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const CYAN: &str = "\x1b[36m";
const YELLOW: &str = "\x1b[33m";
const GREEN: &str = "\x1b[32m";

static COLOR_ENABLED: OnceLock<bool> = OnceLock::new();

/// Resolve whether output should be colorized from the three inputs that can
/// control it.
///
/// Priority (highest first): `no_color`, `force_color`, then
/// `disable_color_flag`.
fn resolve_color_enabled(no_color: bool, force_color: bool, disable_color_flag: bool) -> bool {
    if no_color {
        false
    } else if force_color {
        true
    } else {
        !disable_color_flag
    }
}

/// Decide whether output should be colorized and latch the result.
///
/// Priority (highest first): `NO_COLOR` env var, `FORCE_COLOR` env var,
/// then the `--disable-color` flag.
pub fn init(disable_color_flag: bool) {
    let enabled = resolve_color_enabled(
        std::env::var_os("NO_COLOR").is_some(),
        std::env::var_os("FORCE_COLOR").is_some(),
        disable_color_flag,
    );
    let _ = COLOR_ENABLED.set(enabled);
}

fn enabled() -> bool {
    *COLOR_ENABLED.get_or_init(|| std::env::var_os("NO_COLOR").is_none())
}

fn colorize(color: &str, s: &str) -> String {
    if enabled() {
        format!("{color}{s}{RESET}")
    } else {
        s.to_string()
    }
}

pub fn bold(s: &str) -> String {
    colorize(BOLD, s)
}
pub fn cyan(s: &str) -> String {
    colorize(CYAN, s)
}
pub fn yellow(s: &str) -> String {
    colorize(YELLOW, s)
}
pub fn green(s: &str) -> String {
    colorize(GREEN, s)
}

pub fn field_line(indent: &str, name: &str, typ: &str) -> String {
    format!("{indent}{} {}\n", yellow(name), green(typ))
}

#[cfg(test)]
mod tests {
    use super::resolve_color_enabled;

    #[test]
    fn defaults_to_enabled() {
        assert!(resolve_color_enabled(false, false, false));
    }

    #[test]
    fn disable_color_flag_disables() {
        assert!(!resolve_color_enabled(false, false, true));
    }

    #[test]
    fn no_color_disables_even_without_flag() {
        assert!(!resolve_color_enabled(true, false, false));
    }

    #[test]
    fn force_color_enables_over_disable_flag() {
        assert!(resolve_color_enabled(false, true, true));
    }

    #[test]
    fn no_color_wins_over_force_color() {
        assert!(!resolve_color_enabled(true, true, false));
    }

    #[test]
    fn no_color_wins_over_force_color_and_disable_flag() {
        assert!(!resolve_color_enabled(true, true, true));
    }
}
