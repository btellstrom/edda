//! What every service's OpenAPI description shares: the token routes need,
//! and the error responses any route may give.

use utoipa::{
    Modify, PartialSchema, ToSchema,
    openapi::{
        OpenApi, Ref, RefOr, ResponseBuilder, SecurityRequirement,
        content::Content,
        path::Operation,
        security::{Http, HttpAuthScheme, SecurityScheme},
    },
};

/// An error, from any route: `{"error": "..."}`.
#[derive(ToSchema)]
pub struct ErrorBody {
    /// What went wrong, for people.
    pub error: String,
}

/// The name of the token in [`Conventions`]' security scheme.
pub const TOKEN: &str = "token";

/// Applies the conventions every service keeps to its description: a
/// route needs a token unless it says otherwise (`security(())`), and then
/// may answer `401`; any route may answer `429`, over the limits; and
/// operations are named after their tag and handler (`combats.list`), as
/// handlers' names repeat.
pub struct Conventions;

impl Modify for Conventions {
    fn modify(&self, openapi: &mut OpenApi) {
        let components = openapi.components.get_or_insert_with(Default::default);
        components.add_security_scheme(
            TOKEN,
            SecurityScheme::Http(
                Http::builder()
                    .scheme(HttpAuthScheme::Bearer)
                    .bearer_format("PASETO v4.public")
                    .description(Some(
                        "From the auth service's `POST /login`; valid for 15 minutes.",
                    ))
                    .build(),
            ),
        );
        components
            .schemas
            .insert("ErrorBody".into(), ErrorBody::schema());
        for item in openapi.paths.paths.values_mut() {
            for operation in [
                &mut item.get,
                &mut item.put,
                &mut item.post,
                &mut item.delete,
            ]
            .into_iter()
            .flatten()
            {
                conventions(operation);
            }
        }
    }
}

fn conventions(operation: &mut Operation) {
    if let (Some(id), Some([tag, ..])) = (&operation.operation_id, operation.tags.as_deref())
        && id != tag
    {
        operation.operation_id = Some(format!("{tag}.{id}"));
    }
    if operation.security.is_none() {
        operation.security = Some(vec![SecurityRequirement::new::<_, [&str; 0], &str>(
            TOKEN,
            [],
        )]);
        add_error(operation, "401", "No token, or not a valid one.");
    }
    add_error(
        operation,
        "429",
        "Too many requests from this address; `Retry-After` says when to try again.",
    );
}

/// An error response, unless the route describes that status itself.
pub fn add_error(operation: &mut Operation, status: &str, description: &str) {
    operation
        .responses
        .responses
        .entry(status.into())
        .or_insert_with(|| error(description));
}

/// An error response with `description`.
pub fn error(description: &str) -> RefOr<utoipa::openapi::Response> {
    ResponseBuilder::new()
        .description(description)
        .content(
            "application/json",
            Content::new(Some(Ref::from_schema_name(ErrorBody::name()))),
        )
        .into()
}
