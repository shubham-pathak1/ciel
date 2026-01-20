//! Ciel Download Manager - Application Entry Point
//! 
//! This is the binary entry point for the Ciel application. 
//! It delegates execution to the `ciel_lib` library where the actual
//! Tauri setup and lifecycle management occur.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // Bootstrap the application by calling the entry point in the library.
    // This separation allows for easier testing and modularity.
    ciel_lib::run()
}
