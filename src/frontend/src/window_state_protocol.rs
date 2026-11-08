//! Public read-only window-state D-Bus contract; docs/window-state.md describes it.
//! External clients depend on these names: change them only with a new interface version.
pub const WORMINAL_STATE_PATH: &str = "/org/worminal/Windows";
pub const WORMINAL_STATE_INTERFACE: &str = "org.worminal.Windows1";
pub const WORMINAL_STATE_WINDOWS: &str = "a(ubbays)";
pub const WORMINAL_STATE_PREFIX: &str = "org.worminal.Instance.p";
pub const WORMINAL_STATE_METHOD: &str = "GetWindows";
pub const WORMINAL_STATE_SIGNAL: &str = "Changed";

pub fn introspection() -> String {
    format!(
        "<node><interface name='{WORMINAL_STATE_INTERFACE}'>\
         <method name='{WORMINAL_STATE_METHOD}'><arg direction='out' type='{WORMINAL_STATE_WINDOWS}'/></method>\
         <signal name='{WORMINAL_STATE_SIGNAL}'><arg type='{WORMINAL_STATE_WINDOWS}'/></signal>\
         </interface></node>"
    )
}

#[cfg(test)]
mod tests {
    /// The introspection data is public; it must match what released clients were built against.
    #[test]
    fn introspection_is_the_published_contract() {
        assert_eq!(
            super::introspection(),
            "<node><interface name='org.worminal.Windows1'>\
             <method name='GetWindows'><arg direction='out' type='a(ubbays)'/></method>\
             <signal name='Changed'><arg type='a(ubbays)'/></signal></interface></node>"
        );
    }
}
