//! The one Hyprland command the anchor runs on the login path besides
//! `clients -j`: place a just-launched window — move it to its pinned
//! workspace without the view following it, and float or tile it as it was
//! captured.
//!
//! It goes through `hyprctl eval`, so the Lua is built here and nowhere else.
//! The only values in it are the window's address, the workspace number and
//! one of two fixed float actions. Hyprland 0.56 reads any other float action
//! as a toggle, so only `enable` and `disable` are ever written.
//! The address is read back from `hyprctl clients -j`, but it is still checked
//! to be `0x` and lowercase hex before it is placed between quotes, so nothing
//! else can ever reach the evaluated text.

use std::fmt::Write as _;

/// Longest address accepted: `0x` and a 64-bit pointer's hex digits.
const MAX_ADDRESS_LEN: usize = 2 + 16;

/// Whether `address` is a window address: `0x` then 1–16 lowercase hex digits.
#[must_use]
pub fn is_address(address: &str) -> bool {
    address.len() <= MAX_ADDRESS_LEN
        && address.strip_prefix("0x").is_some_and(|hex| {
            !hex.is_empty() && hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        })
}

/// The Lua that places the window at `address`: moved to `workspace` when
/// given, floated (`Some(true)`) or tiled (`Some(false)`) when given. `None`
/// when the address is not one, a workspace is below 1, or there is nothing
/// to do.
#[must_use]
pub fn place_lua(address: &str, workspace: Option<u32>, float: Option<bool>) -> Option<String> {
    if !is_address(address) || workspace.is_some_and(|ws| ws < 1) {
        return None;
    }
    let mut steps = String::new();
    if let Some(ws) = workspace {
        let _ = write!(
            steps,
            " hl.dispatch(hl.dsp.window.move({{window=w, workspace=\"{ws}\", follow=false}}))"
        );
    }
    if let Some(f) = float {
        let action = if f { "enable" } else { "disable" };
        let _ = write!(
            steps,
            " hl.dispatch(hl.dsp.window.float({{window=w, action=\"{action}\"}}))"
        );
    }
    if steps.is_empty() {
        return None;
    }
    Some(format!(
        "for _,w in ipairs(hl.get_windows()) do if w.address==\"{address}\" then{steps} end end"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_lowercase_hex_address_is_accepted() {
        for ok in ["0x0", "0x55d1c0ffee00", "0xffffffffffffffff"] {
            assert!(is_address(ok), "{ok}");
        }
        for bad in [
            "",
            "0x",
            "55d1",
            "0X55",
            "0x55D1",
            "0x55\"",
            "0x55 ",
            "0x55\n",
            "0x1ffffffffffffffff",
            "0x55\") os.execute(\"x",
        ] {
            assert!(!is_address(bad), "{bad:?}");
            assert_eq!(place_lua(bad, Some(1), Some(true)), None, "{bad:?}");
        }
    }

    #[test]
    fn the_lua_names_exactly_the_address_workspace_and_a_fixed_float_action() {
        let head = "for _,w in ipairs(hl.get_windows()) do if w.address==\"0x5a\" then";
        let mv = " hl.dispatch(hl.dsp.window.move({window=w, workspace=\"8\", follow=false}))";
        let fl =
            |a: &str| format!(" hl.dispatch(hl.dsp.window.float({{window=w, action=\"{a}\"}}))");
        assert_eq!(
            place_lua("0x5a", Some(8), None),
            Some(format!("{head}{mv} end end"))
        );
        assert_eq!(
            place_lua("0x5a", Some(8), Some(true)),
            Some(format!("{head}{mv}{} end end", fl("enable")))
        );
        assert_eq!(
            place_lua("0x5a", None, Some(false)),
            Some(format!("{head}{} end end", fl("disable")))
        );
        assert_eq!(
            place_lua("0x5a", Some(0), None),
            None,
            "workspace 0 is not one"
        );
        assert_eq!(place_lua("0x5a", None, None), None, "nothing to do");
    }
}
