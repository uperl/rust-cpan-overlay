//! Minimal homepage. The full website and API are milestone 3; this page
//! only has to make clear that this is an overlay and how to use it.

use crate::manifest::{Kind, Manifest};

pub fn render(manifest: &Manifest) -> String {
    let title = manifest.site.title.as_deref().unwrap_or("CPAN Overlay");
    let url = manifest
        .site
        .public_url
        .as_deref()
        .map(|u| u.trim_end_matches('/').to_string())
        .unwrap_or_else(|| "https://this-overlay.example".to_string());

    let mut html = String::new();
    html.push_str("<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\">");
    html.push_str("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">");
    html.push_str(&format!("<title>{}</title>", esc(title)));
    html.push_str(
        "<style>body{font-family:system-ui,sans-serif;max-width:50rem;margin:2rem auto;padding:0 1rem;line-height:1.5}\
         code,pre{background:#f3f3f3;padding:.1rem .3rem}pre{padding:.6rem;overflow-x:auto}\
         @media (prefers-color-scheme:dark){body{background:#111;color:#ddd}code,pre{background:#222}}</style>",
    );
    html.push_str("</head><body>");
    html.push_str(&format!("<h1>{}</h1>", esc(title)));
    if let Some(desc) = &manifest.site.description {
        html.push_str(&format!("<p>{}</p>", esc(desc)));
    }
    html.push_str(
        "<p><strong>This is a CPAN overlay, not CPAN itself.</strong> It serves the public CPAN \
         index with the explicit amendments listed below; everything else comes from upstream.</p>",
    );

    html.push_str("<h2>Use it</h2><pre>");
    html.push_str(&esc(&format!("cpanm --from {url} Some::Module")));
    html.push_str("</pre>");

    for (kind, heading) in [
        (Kind::Patched, "Patched distributions"),
        (Kind::Personal, "Personal distributions"),
    ] {
        let dists: Vec<_> = manifest
            .distributions
            .iter()
            .filter(|d| d.kind == kind)
            .collect();
        if dists.is_empty() {
            continue;
        }
        html.push_str(&format!("<h2>{heading}</h2><ul>"));
        for d in dists {
            html.push_str(&format!(
                "<li><a href=\"/authors/id/{}\">{} {}</a>",
                esc(&d.path),
                esc(&d.name),
                esc(&d.version)
            ));
            if let Some(base) = &d.upstream {
                html.push_str(&format!(" (replaces upstream {})", esc(&base.version)));
            }
            if !d.patches.is_empty() {
                html.push_str("<ul>");
                for p in &d.patches {
                    html.push_str(&format!("<li>{}</li>", esc(&p.summary)));
                }
                html.push_str("</ul>");
            }
            html.push_str("</li>");
        }
        html.push_str("</ul>");
    }
    html.push_str(&format!(
        "<p><small>CPAN Overlay {}</small></p></body></html>\n",
        crate::VERSION
    ));
    html
}

fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_metadata() {
        let mut v: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/manifest.json")).unwrap();
        v["site"]["title"] = "<script>x</script>".into();
        let m = Manifest::from_json(&v.to_string()).unwrap();
        let html = render(&m);
        assert!(!html.contains("<script>"));
        assert!(html.contains("&lt;script&gt;"));
        assert!(html.contains("cpanm --from https://cpan.example.com Some::Module"));
        assert!(html.contains("Fix Perl 5.42 compatibility"));
    }
}
