//! Generated Rust source for editor and parser benchmarks.

pub fn big_rust(lines: usize) -> String {
    let mut source = String::new();
    let mut index = 0;
    let mut generated_lines = 0;
    while generated_lines < lines {
        source.push_str(&format!(
            "/// Generated service {index}.\npub struct Service{index} {{ value: usize }}\nimpl Service{index} {{\n    pub fn value(&self) -> usize {{\n        self.value + {index}\n    }}\n}}\n"
        ));
        index += 1;
        generated_lines += 7;
    }
    source
}
