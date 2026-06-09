//! Compressor for `make` / `gmake` output.
//!
//! Strips "Entering/Leaving directory" lines and condenses successful
//! no-op runs to a single summary line. Preserves actual command lines
//! and error messages.

use super::BashCompressor;

pub struct MakeCompressor;

impl BashCompressor for MakeCompressor {
    fn compress(&self, _command: &str, output: &str) -> Option<String> {
        if output.trim().is_empty() {
            return None;
        }

        let compressed = compress_make_output(output);
        if compressed.trim_end().len() >= output.trim_end().len() {
            return None;
        }

        Some(compressed)
    }
}

/// Returns true for lines that indicate make target failure.
fn is_error_line(line: &str) -> bool {
    // "make[1]: *** [Makefile:10: target] Error 1"
    // "make: *** [Makefile:5: all] Error 2"
    // "make: *** No rule to make target"
    (line.contains("***") && (line.contains("Error ") || line.contains("No rule to make")))
        || line.contains("Stop.")
        || (line.contains(": error:") && !line.starts_with("make[") && !line.starts_with("make:"))
        || (line.contains(": fatal error:"))
}

fn compress_make_output(output: &str) -> String {
    let mut result: Vec<&str> = Vec::new();
    let mut has_errors = false;

    for line in output.lines() {
        // Strip "make[N]: Entering/Leaving directory '...'"
        if (line.starts_with("make[") || line.starts_with("make:"))
            && (line.contains("]: Entering directory")
                || line.contains("]: Leaving directory")
                || line.contains(": Entering directory")
                || line.contains(": Leaving directory"))
        {
            continue;
        }

        // Skip blank lines
        if line.trim().is_empty() {
            continue;
        }

        if is_error_line(line) {
            has_errors = true;
        }

        // "Nothing to be done" and "up to date" are noise when there are no errors
        let is_noop = line.contains("Nothing to be done for") || line.contains("] is up to date");
        if is_noop && !has_errors {
            // Keep track but don't add to output yet
            continue;
        }

        result.push(line);
    }

    // If everything was stripped and no errors: emit compact summary
    if result.is_empty() {
        return "make: all targets up to date".to_string();
    }

    // If only "nothing to be done" noise remained, already handled above
    result.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strips_entering_leaving() {
        let output = "make[1]: Entering directory '/home/user/project'\ngcc -o main main.c\nmake[1]: Leaving directory '/home/user/project'\n";
        let result = MakeCompressor.compress("make", output).unwrap();
        assert!(!result.contains("Entering directory"));
        assert!(!result.contains("Leaving directory"));
        assert!(result.contains("gcc -o main main.c"));
    }

    #[test]
    fn test_nothing_to_do_summarised() {
        let output = "make[1]: Entering directory '/project'\nmake[1]: Nothing to be done for 'all'.\nmake[1]: Leaving directory '/project'\n";
        let result = MakeCompressor.compress("make", output).unwrap();
        assert_eq!(result.trim(), "make: all targets up to date");
    }

    #[test]
    fn test_errors_kept() {
        let output = "make[1]: Entering directory '/project'\ngcc -o main main.c\nmain.c:5:1: error: expected ';'\nmake[1]: *** [Makefile:10: main] Error 1\nmake[1]: Leaving directory '/project'\nmake: *** [Makefile:5: all] Error 2\n";
        let result = MakeCompressor.compress("make", output).unwrap();
        assert!(result.contains("main.c:5:1: error"));
        assert!(result.contains("Error 1"));
        assert!(result.contains("Error 2"));
        assert!(!result.contains("Entering directory"));
    }

    #[test]
    fn test_empty_passthrough() {
        assert!(MakeCompressor.compress("make", "").is_none());
    }

    #[test]
    fn test_multiple_subdirs_stripped() {
        let output = "make[1]: Entering directory '/project/src'\ngcc -c foo.c\nmake[1]: Leaving directory '/project/src'\nmake[2]: Entering directory '/project/lib'\nar rcs libfoo.a foo.o\nmake[2]: Leaving directory '/project/lib'\n";
        let result = MakeCompressor.compress("make all", output).unwrap();
        assert!(!result.contains("Entering"));
        assert!(result.contains("gcc -c foo.c"));
        assert!(result.contains("ar rcs libfoo.a"));
    }

    #[test]
    fn test_is_error_line_detection() {
        assert!(is_error_line("make[1]: *** [Makefile:10: target] Error 1"));
        assert!(is_error_line("make: *** No rule to make target 'foo'"));
        assert!(is_error_line("make: Stop."));
        assert!(!is_error_line("gcc -o main main.c"));
        assert!(!is_error_line("make[1]: Nothing to be done for 'all'."));
    }
}
