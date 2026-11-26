//! `CHECKSUMS` for the overlay author directory, in the Data::Dumper layout
//! PAUSE uses (read by CPAN.pm through `Safe`).
//!
//! Every configured artifact is listed, active or not, because artifact
//! paths stay downloadable after an override is deactivated. The file is
//! not signed; upstream signatures are never copied.

use crate::manifest::Manifest;

pub fn render(manifest: &Manifest) -> String {
    let mut dists: Vec<_> = manifest.distributions.iter().collect();
    dists.sort_by(|a, b| a.file_name().cmp(b.file_name()));

    let mut out = String::new();
    out.push_str(&format!(
        "# CHECKSUMS file written by CPAN Overlay {}\n",
        crate::VERSION
    ));
    out.push_str("# This file is not signed.\n");
    out.push_str("$cksum = {\n");
    for d in dists {
        // File names, digests and dates are validated to a character set
        // that needs no quoting inside single-quoted Perl strings.
        let mut fields = Vec::new();
        if let Some(md5) = &d.md5 {
            fields.push(format!("    'md5' => '{md5}'"));
        }
        if let Some(mtime) = &d.mtime {
            fields.push(format!("    'mtime' => '{mtime}'"));
        }
        fields.push(format!("    'sha256' => '{}'", d.sha256));
        if let Some(size) = d.size {
            fields.push(format!("    'size' => {size}"));
        }
        out.push_str(&format!("  '{}' => {{\n", d.file_name()));
        out.push_str(&fields.join(",\n"));
        out.push_str("\n  },\n");
    }
    out.push_str("};\n__END__\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_all_artifacts() {
        let m = Manifest::from_json(include_str!("../tests/fixtures/manifest.json")).unwrap();
        let text = render(&m);
        let foo = text.find("'Foo-Bar-1.24.1.tar.gz' => {").unwrap();
        let personal = text.find("'Personal-Tools-0.01.tar.gz' => {").unwrap();
        assert!(foo < personal);
        assert!(text.contains(&format!("'sha256' => '{}'", m.distributions[0].sha256)));
        assert!(text.contains("'size' => 1234"));
        assert!(text.trim_end().ends_with("};\n__END__"));
    }
}
