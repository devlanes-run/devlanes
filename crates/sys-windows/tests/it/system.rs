use devlanes_sys_windows::system::computer_name;

/// The name comes back non-empty and without the NUL Windows writes after it.
#[test]
fn computer_name_is_read() {
    let name = computer_name().expect("computer name");
    assert!(!name.is_empty());
    assert!(!name.contains('\0'), "{name:?}");
}
