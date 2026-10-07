use authz::Role;
use authz::authorizers::SystemAuthorizer;
use authz::v2::group_members;
use authz::v2::user_groups;
use axum::Extension;
use axum::extract::Json;
use axum::extract::Query;
use axum::extract::State;
use editoast_derive::EditoastError;
use models::StdcmRequest;
use models::stdcm_request::PaginatedList;
use models::stdcm_request::SelectionCursor;
use models::stdcm_request::SelectionSettings;
use serde::Deserialize;
use serde::Serialize;
use thiserror::Error;
use utoipa::IntoParams;
use utoipa::ToSchema;

use crate::AppState;
use crate::error::Result;
use crate::views::AuthorizationError;

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum IdentityFilter {
    Group,
    User,
}

#[derive(IntoParams, Deserialize)]
#[into_params(parameter_in = Query)]
pub struct PaginationQueryParams {
    after: Option<String>,
    before: Option<String>,
    size: Option<usize>,
}

#[derive(IntoParams, Deserialize)]
#[into_params(parameter_in = Query)]
pub(in crate::views) struct ListStdcmRequestQueryParams {
    #[param(inline)]
    by: Option<IdentityFilter>,
}

#[derive(Serialize, ToSchema)]
pub struct Links {
    prev: Option<String>,
    next: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct ListStdcmRequestResponse {
    #[schema(inline)]
    links: Links,
    requests: Vec<StdcmRequest>,
}

impl From<PaginatedList> for ListStdcmRequestResponse {
    fn from(list: PaginatedList) -> Self {
        ListStdcmRequestResponse {
            requests: list.requests,
            links: Links {
                prev: list.prev.map(|cursor| cursor.to_string()),
                next: list.next.map(|cursor| cursor.to_string()),
            },
        }
    }
}

#[derive(Debug, Error, EditoastError, derive_more::From)]
#[editoast_error(base_id = "stdcm_request")]
pub enum StdcmRequestError {
    #[error("Malformed parameters")]
    #[editoast_error(status = 400)]
    MalformedParameters,
    #[error("Specifying both after and before cursor is not supported")]
    #[editoast_error(status = 400)]
    RangePaginationNotSupported,
    #[error("Endpoint requires user authentication and is not available to backend services")]
    #[editoast_error(status = 401)]
    NotUser,
    #[error("Endpoint only available to backend services")]
    #[editoast_error(status = 403)]
    Forbidden,
    #[error(transparent)]
    #[editoast_error(status = 500)]
    #[from(models::Error, database::DatabaseError)]
    Database(models::Error),
}

/// Return a list of stdcm requests
#[editoast_derive::route(Role::OperationalStudies)]
#[utoipa::path(
    get, path = "",
    tag = "scenarios",
    params(ListStdcmRequestQueryParams, PaginationQueryParams),
    responses(
        (status = 200, body = ListStdcmRequestResponse, description = "List of stdcm requests"),
    )
)]
pub(in crate::views) async fn list(
    State(AppState {
        openfga, db_pool, ..
    }): State<AppState>,
    Query(list_stdcm_request_query_params): Query<ListStdcmRequestQueryParams>,
    Query(pagination_params): Query<PaginationQueryParams>,
    Extension(authn_state): Extension<crate::authentication::State>,
) -> Result<Json<ListStdcmRequestResponse>> {
    let mut settings = SelectionSettings::new();
    if pagination_params.after.is_some() && pagination_params.before.is_some() {
        return Err(StdcmRequestError::RangePaginationNotSupported.into());
    }
    if let Some(size) = pagination_params.size {
        settings = settings.limit(size)
    };
    if let Some(after) = pagination_params.after {
        settings = settings.cursor(SelectionCursor::AfterCursor(
            after
                .parse::<i64>()
                .map_err(|_| StdcmRequestError::MalformedParameters)?,
        ))
    };
    if let Some(before) = pagination_params.before {
        settings = settings.cursor(SelectionCursor::BeforeCursor(
            before
                .parse::<i64>()
                .map_err(|_| StdcmRequestError::MalformedParameters)?,
        ))
    };
    if let Some(user) = authn_state.user() {
        match list_stdcm_request_query_params.by {
            Some(IdentityFilter::User) => settings = settings.user_filter(vec![user.0]),
            Some(IdentityFilter::Group) => {
                let system = SystemAuthorizer::new_infallible(&openfga);
                let groups = user_groups(user)
                    .run::<AuthorizationError, _>(&system)
                    .await?;
                let mut same_group_user_ids = vec![];
                for group in groups {
                    same_group_user_ids.extend(
                        (group_members(group))
                            .run::<AuthorizationError, _>(&system)
                            .await?
                            .into_iter()
                            .map(|user| user.0),
                    )
                }
                settings = settings.user_filter(same_group_user_ids)
            }
            None if authn_state.roles().contains(&Role::Admin) => (),
            None => return Err(StdcmRequestError::Forbidden.into()),
        }
    } else if list_stdcm_request_query_params.by.is_some() {
        return Err(StdcmRequestError::NotUser.into());
    }

    let settings = settings;
    let mut conn = db_pool.get().await?;
    Ok(Json(StdcmRequest::list(&mut conn, settings).await?.into()))
}
