use std::collections::HashSet;
use std::io::ErrorKind;
use std::path::Path;

use rhaf_api::models::{Catalog, CatalogStatus, Product};
use serde::Deserialize;
use url::Url;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CatalogFile {
    products: Vec<Product>,
}

pub fn load(path: Option<&Path>) -> Catalog {
    let Some(path) = path else {
        return absent();
    };
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) if error.kind() == ErrorKind::NotFound => return absent(),
        Err(_) => return invalid("Cannot read product catalog"),
    };
    parse(&content)
}

fn parse(content: &str) -> Catalog {
    let Ok(file) = serde_yaml::from_str::<CatalogFile>(content) else {
        return invalid("Catalog validation failed");
    };
    let mut ids = HashSet::new();
    if !file.products.iter().all(|product| {
        !product.id.trim().is_empty()
            && !product.display_name.trim().is_empty()
            && ids.insert(product.id.as_str())
            && product
                .links
                .iter()
                .all(|link| !link.label.trim().is_empty() && valid_url(&link.url))
    }) {
        return invalid("Catalog validation failed");
    }
    Catalog {
        status: CatalogStatus::Loaded,
        products: file.products,
        message: None,
    }
}

fn valid_url(value: &str) -> bool {
    let Some((scheme, rest)) = value.split_once("://") else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    (scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https"))
        && !authority.is_empty()
        && !authority.contains('@')
        && !value.chars().any(|character| {
            character.is_whitespace() || character.is_control() || character == '\\'
        })
        && Url::parse(value).is_ok_and(|url| url.has_host())
}

const fn absent() -> Catalog {
    Catalog {
        status: CatalogStatus::Absent,
        products: Vec::new(),
        message: None,
    }
}

fn invalid(message: &str) -> Catalog {
    Catalog {
        status: CatalogStatus::Invalid,
        products: Vec::new(),
        message: Some(message.to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::{CatalogStatus, load, parse};

    #[test]
    fn loads_products_and_optional_links() -> anyhow::Result<()> {
        let file = tempfile::NamedTempFile::new()?;
        std::fs::write(
            file.path(),
            "products: [{id: one, displayName: One}, {id: two, displayName: Two, links: [{label: Docs, url: HtTpS://example.com/path@ref}]}]",
        )?;
        let catalog = load(Some(file.path()));
        assert!(matches!(catalog.status, CatalogStatus::Loaded));
        assert_eq!(catalog.products.len(), 2);
        assert!(catalog.products[0].links.is_empty());
        assert_eq!(catalog.products[1].links[0].label, "Docs");
        assert!(catalog.message.is_none());
        Ok(())
    }

    #[test]
    fn rejects_the_entire_invalid_catalog_with_static_diagnostics() {
        for input in [
            "products: [HIDDEN",
            "products: [{id: a, displayName: One}, {id: a, displayName: Two}]",
            "products: [{id: ' ', displayName: Empty ID}]",
            "products: [{id: a}]",
            "products: [{id: a, displayName: ' '}]",
            "products: [{id: a, displayName: One, links: [{label: ' ', url: https://example.com}]}]",
            "products: [{id: a, displayName: One, links: [{label: Docs, url: https://user:HIDDEN@example.com}]}]",
            "products: [{id: a, displayName: One, links: [{label: Docs, url: https://@example.com}]}]",
        ] {
            let catalog = parse(input);
            assert!(matches!(catalog.status, CatalogStatus::Invalid));
            assert!(catalog.products.is_empty());
            assert_eq!(
                catalog.message.as_deref(),
                Some("Catalog validation failed")
            );
        }
    }

    #[test]
    fn requires_absolute_http_urls() {
        for url in [
            "/relative",
            "//example.com",
            "ftp://example.com",
            "https:example.com",
            "https:///example.com",
            "https://",
            "https://example.com:invalid",
        ] {
            assert!(!super::valid_url(url));
        }
    }

    #[test]
    fn reports_missing_and_unreadable_catalogs() -> anyhow::Result<()> {
        let directory = tempfile::tempdir()?;
        let absent = load(Some(&directory.path().join("missing.yaml")));
        assert!(matches!(absent.status, CatalogStatus::Absent));
        assert!(absent.products.is_empty());
        assert!(absent.message.is_none());
        let invalid = load(Some(directory.path()));
        assert!(matches!(invalid.status, CatalogStatus::Invalid));
        assert!(invalid.products.is_empty());
        assert_eq!(
            invalid.message.as_deref(),
            Some("Cannot read product catalog")
        );
        Ok(())
    }
}
