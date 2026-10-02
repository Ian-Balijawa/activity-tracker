# Activity Tracker

Pulls your GitHub work for an organization over a date range. Returns commits and pull requests grouped by repository. Only repositories you contributed to appear.

## Run

```
cp .env.example .env
cargo run
```

Server starts on http://localhost:3000.

## Token

Create a personal access token and send it with each request.

- Classic token scopes: `repo`, `read:org`
- Fine-grained token: select the org, then grant read access to Metadata, Contents and Pull requests
- If the org uses SAML SSO, authorize the token for that org

Token lookup order:

- Header `X-GitHub-Token`
- Header `Authorization: Bearer <token>`
- `GITHUB_TOKEN` in `.env`

## Endpoints

- `GET /health`
- `GET /api/v1/orgs/{org}/activity`

Query parameters for the activity endpoint:

- `month`: whole month, like `2026-09`
- `from`, `to`: inclusive dates, like `2026-09-01`. `to` defaults to today
- `author`: GitHub login. Defaults to the owner of the token
- `include_commit_stats`: `true` adds lines added and removed per commit. Slower
- `exclude_merges`: `true` drops merge commits

With no date parameters, it returns the current month so far. Ranges are capped at 366 days.

## How it works

- Lists org repositories pushed to since the start date
- Fetches your commits in each one, 8 requests at a time
- Searches the org for your pull requests created or merged in the range
- Loads each pull request for line counts, branches and merge state
- Drops repositories with no activity

## Limits

- Commits come from the default branch of each repository
- Commits only match when the commit email is linked to your GitHub account
- GitHub search returns at most 1000 results per query
- Search allows 30 requests per minute

## Postman

Import `postman/committer.postman_collection.json`. Set the collection variables `github_token` and `org`.

## Tests

```
cargo test
```
