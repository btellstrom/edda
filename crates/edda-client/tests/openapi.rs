#![allow(clippy::unwrap_used)] // tests panic on failure, which is the point

//! `docs/openapi.json` describes the four services as they are: one
//! description, as behind the HTTPS proxy they share one address. When a
//! route changes, `make openapi` writes it anew.

use std::{env, fs, path::Path};

use utoipa::{
    OpenApi,
    openapi::{
        InfoBuilder, LicenseBuilder, OpenApi as Description, SecurityRequirement, ServerBuilder,
        server::ServerVariableBuilder,
    },
};

fn description() -> Description {
    let mut api = auth_service::ApiDoc::openapi();
    api.merge(character_service::ApiDoc::openapi());
    api.merge(combat_service::ApiDoc::openapi());
    api.merge(monster_service::ApiDoc::openapi());
    api.info = InfoBuilder::new()
        .title("Edda")
        .version(env!("CARGO_PKG_VERSION"))
        .description(Some(
            "The services behind Edda: auth, characters, combat and monsters. \
             Behind the HTTPS proxy they share one address; run on their own, \
             each has its own (see docs/api.md). Every route but a few needs a \
             token from `POST /login`.",
        ))
        .license(Some(
            LicenseBuilder::new()
                .name("MIT OR Apache-2.0")
                .identifier(Some("MIT OR Apache-2.0"))
                .build(),
        ))
        .build();
    api.servers = Some(vec![
        ServerBuilder::new()
            .url("https://{host}")
            .description(Some("Behind the HTTPS proxy, e.g. dnd.example.com:8443"))
            .parameter(
                "host",
                ServerVariableBuilder::new().default_value("localhost:8443"),
            )
            .build(),
    ]);
    api
}

#[test]
fn docs_describe_the_services_as_they_are() {
    let written = description().to_pretty_json().unwrap() + "\n";
    let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/openapi.json");
    if env::var_os("UPDATE_OPENAPI").is_some() {
        fs::write(&file, &written).unwrap();
    }
    let kept = fs::read_to_string(&file).unwrap_or_default();
    assert!(
        kept == written,
        "docs/openapi.json is out of date: `make openapi` writes it anew"
    );
}

#[test]
fn every_route_says_what_it_answers() {
    let api = description();
    for (path, item) in &api.paths.paths {
        for operation in [&item.get, &item.put, &item.post, &item.delete]
            .into_iter()
            .flatten()
        {
            assert!(
                operation
                    .responses
                    .responses
                    .keys()
                    .any(|status| status.starts_with('2') || status == "101"),
                "{path} has no success response"
            );
            assert!(operation.tags.is_some(), "{path} has no tag");
        }
    }
    // Signing in needs no token; the rest does.
    let needs_token = |path: &str| {
        let item = &api.paths.paths[path];
        let operation = item.get.as_ref().or(item.post.as_ref()).unwrap();
        operation
            .security
            .iter()
            .flatten()
            .any(|requirement| *requirement != SecurityRequirement::default())
    };
    assert!(!needs_token("/login"));
    assert!(!needs_token("/health"));
    assert!(needs_token("/characters"));
}
