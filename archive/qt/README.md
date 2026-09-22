# Archived Qt interface

The former interface uses PySide6 (Qt 6). It is archived here and is not shipped or imported by the Tauri application. Its modules and assets are retained for comparison and existing backend regression fixtures. The isolated legacy test runner adds this directory to the package path; production code does not.

The supported launcher is the repository's Start.command, which launches Tauri. Do not run both interfaces against the live database at the same time.
