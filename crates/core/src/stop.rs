//! Stop signals as `stop.signal` names them. Runners map them to what their OS
//! delivers: the POSIX signal on Unix, a console event or termination on Windows.

/// A signal `stop.signal` accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StopSignal {
    Hangup,
    Interrupt,
    Quit,
    Kill,
    Terminate,
    User1,
    User2,
}

impl StopSignal {
    /// Every signal, in config-name order.
    pub const ALL: [Self; 7] = [
        Self::Hangup,
        Self::Interrupt,
        Self::Quit,
        Self::Kill,
        Self::Terminate,
        Self::User1,
        Self::User2,
    ];

    /// Parses a config name (`SIGTERM`); `None` for signals config does not accept.
    ///
    /// ```
    /// use devlanes_core::StopSignal;
    /// assert_eq!(StopSignal::from_config_name("SIGINT"), Some(StopSignal::Interrupt));
    /// assert_eq!(StopSignal::from_config_name("SIGWINCH"), None);
    /// ```
    pub fn from_config_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.config_name() == name)
    }

    /// The config name, such as `SIGTERM`.
    pub fn config_name(self) -> &'static str {
        match self {
            Self::Hangup => "SIGHUP",
            Self::Interrupt => "SIGINT",
            Self::Quit => "SIGQUIT",
            Self::Kill => "SIGKILL",
            Self::Terminate => "SIGTERM",
            Self::User1 => "SIGUSR1",
            Self::User2 => "SIGUSR2",
        }
    }
}

/// What a Windows stop sends for a configured signal. Windows has no POSIX
/// signals; console processes only see Ctrl-C and Ctrl-Break.
#[cfg(windows)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowsStop {
    /// Ctrl-C. The runner decides at stop time how it goes out: as the key in
    /// a pseudo console, or as Ctrl-Break when an MSYS program runs in a unit
    /// without one (see the runner-windows README).
    CtrlC,
    /// Ctrl-Break to the unit's console; the default handler ends the process
    /// (Node reports `SIGBREAK`).
    CtrlBreak,
    /// Immediate termination of every unit process.
    Terminate,
}

#[cfg(windows)]
impl StopSignal {
    /// The Windows stop for this signal, as far as the config decides it.
    /// SIGINT and SIGTERM are Ctrl-C, which Node, Python and most servers
    /// treat as a graceful stop. SIGHUP, SIGQUIT, SIGUSR1 and SIGUSR2 have no
    /// equivalent and get Ctrl-Break, so a config shared with Unix machines
    /// still works.
    pub fn windows_stop(self) -> WindowsStop {
        match self {
            Self::Interrupt | Self::Terminate => WindowsStop::CtrlC,
            Self::Kill => WindowsStop::Terminate,
            Self::Hangup | Self::Quit | Self::User1 | Self::User2 => WindowsStop::CtrlBreak,
        }
    }

    /// The launch note (`stop_signal_mapped`) for a signal that Windows always
    /// sends as Ctrl-Break; `None` for SIGINT, SIGTERM and SIGKILL.
    pub fn windows_launch_note(self) -> Option<String> {
        (self.windows_stop() == WindowsStop::CtrlBreak).then(|| {
            format!(
                "on Windows stop.signal {} is sent as Ctrl-Break",
                self.config_name()
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    /// Config names round-trip; other names are rejected rather than defaulted.
    fn config_names_round_trip() {
        for signal in StopSignal::ALL {
            assert_eq!(
                StopSignal::from_config_name(signal.config_name()),
                Some(signal)
            );
        }
        for name in ["SIGWINCH", "TERM", "sigterm", "15", ""] {
            assert_eq!(StopSignal::from_config_name(name), None, "{name}");
        }
    }
}
