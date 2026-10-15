// Firestorm starts this executable a second time as its LEAP helper. Keep the
// Windows GUI subsystem in debug builds too, so the helper never opens a
// console window alongside Firestorm. LEAP communicates over the standard
// input/output handles supplied by the viewer.
#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

fn main() {
    shelfmark_lib::run();
}
