// Hide the extra console window on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! The desktop's way in. Everything is in the library beside this file, because a
//! phone has no `main` to call: there the platform loads the same code as a
//! shared object and enters at `run` itself.

fn main() {
    arvolo_gui_lib::run()
}
