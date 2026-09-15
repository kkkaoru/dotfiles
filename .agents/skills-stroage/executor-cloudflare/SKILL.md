---
name: executor-cloudflare
description: Set up and use Cloudflare through Executor integrations rather than direct agent MCP registration. Use for Cloudflare agent setup, accounts, Workers, R2, D1, billing, analytics, OAuth, and MCP connection troubleshooting. Adapts the official Cloudflare agent-setup prompt to Executor and reuses installed Cloudflare skills.
---

# Cloudflare through Executor

## Scope and source

Use Executor as the single integration, connection, credential, and policy manager.
Do not add duplicate MCP configuration to Pi, Codex, Claude, Cursor, or OpenCode.
This skill supplements, rather than replaces, the official Cloudflare product skills.

On setup requests, fetch the current official instructions:
https://developers.cloudflare.com/agent-setup/prompt.md

Adapt the instructions to Executor instead of blindly running another agent's
installation commands. Reuse existing installations and connections. Do not claim
setup is complete merely because an integration is registered.

Read the installed `executor-and-skills` skill first. In Pi its usual location is:
`~/.pi/agent/packages/executor-skills/skills/executor-and-skills/SKILL.md`.
Follow its current discovery, credential handoff, OAuth, and approval guidance.
If it is absent, report the missing prerequisite rather than installing a second daemon.

## Execution and discovery

Use `bash` for bounded local discovery, with a timeout below 30 seconds.
Use `tmux_exec` for network calls and uncertain-duration work; return promptly,
then inspect the output artifact on completion. Never equate CLI exit zero with
success: inspect `ok`, nested `isError`, service errors, and execution pauses.

Start with:

```bash
executor tools integrations
executor tools search 'search_skills' --namespace local-skills --limit 3
```

Describe the exact returned tool path before calling it. Search installed skills
with query `cloudflare`, then discover/describe/call `read_skill` for the selected
ID. Load product references as needed using `read_reference` and its pagination.
Do not replace product-specific instructions with remembered API signatures.

Discover service tools narrowly:

```bash
executor tools search 'search' --namespace cloudflare-api --limit 3
executor tools describe '<exact returned path>'
```

Invoke discovered paths using `executor call <path segments> '<schema-valid JSON>'`.
Never assume `user.default`: connections can have different owners and names.
Never print credentials or read Executor's credential database directly.

## Integration routing

The names below are local conventions, not guaranteed connection paths.
Discover the installed catalog every time context is uncertain.

| Task | Preferred integration | Official endpoint |
| --- | --- | --- |
| Account/resource API, Workers, DNS, R2, D1, KV | `cloudflare-api` | `https://mcp.cloudflare.com/mcp` |
| Current product docs and pricing | `cloudflare-docs` | `https://docs.mcp.cloudflare.com/mcp` |
| Bindings and resource discovery | `cloudflare-bindings` | `https://bindings.mcp.cloudflare.com/mcp` |
| Build/deploy diagnostics | `cloudflare-builds` | `https://builds.mcp.cloudflare.com/mcp` |
| Workers logs and metrics | `cloudflare-observability` | `https://observability.mcp.cloudflare.com/mcp` |
| Analytics and usage | `cloudflare-graphql` | Discover installed integration |
| Browser rendering | `cloudflare-browser` | Discover installed integration |
| Agents SDK documentation | `cloudflare-agents` | Discover installed integration |
| Billing-token connection to the same official API MCP | `cloudflare-billing-mcp`, if installed | `https://mcp.cloudflare.com/mcp` |

Browser rendering is not automatically an authenticated Dashboard browser session.
Bindings/builds/observability tools do not automatically expose invoice amounts.

## Setup and verification

1. Fetch the official setup prompt and inventory existing integrations and skills.
2. If official Cloudflare skills are missing and installation was requested, use
   Bun rather than npm: `bunx --bun skills add cloudflare/skills --skill '*' --yes --global`.
   Run this as detached network work. Do not reinstall over existing skills without
   checking what would change.
3. For missing integrations, discover and describe `executor.mcp.addServer`.
   Register only the requested missing services, using official HTTPS endpoints.
   Cloudflare Docs needs no authentication; protected services need a connection.
4. For connection inspection, discover and describe
   `executor.coreTools.connections.list`. Its verbose form exposes scope names,
   not secret values. A healthy credential-only check is not a live API probe.
5. For OAuth, read the local Executor setup guide and use its
   `executor-cloudflare-auth.sh` helper. Resolve the helper from the actual
   installed dotfiles checkout; do not hardcode a username or checkout path.
   Human login, account selection, and consent remain human actions.
6. For API tokens, use `connections.createHandoff`. The user enters secrets in
   Executor's browser UI, never chat, shell arguments, repository files, or logs.
7. After human completion, list connections to discover the actual owner/name;
   refresh that connection when needed and inspect health and produced tools.
8. Make a bounded read-only live call, such as listing accounts, and verify the
   response. Report verified services separately from untested services.

Do not weaken approval policies. Follow the installed Executor guidance for a
paused execution: show exact action/arguments and approval URL, and resume only
with explicit approval covering that execution ID. Never auto-accept a second
nested prompt. Automatic tool approval does not authorize unrelated cloud writes.

The global skill directory is `~/.agents/skills/executor-cloudflare/`. Pi discovers
it on reload or restart. Do not claim that direct MCP configuration files were
created when all tools are accessed through Executor.

## Cloudflare API Code Mode

Discover/describe `search`, search the OpenAPI spec for the endpoint, and inspect
only its parameters and relevant response fields. Then discover/describe `execute`
and call `cloudflare.request` from an async arrow function.

- Pass the user's account ID explicitly; never silently choose another account.
- For investigations, use GET and bounded pagination. GraphQL read queries may
  use POST, but are not mutations.
- Request only necessary fields. Filter bulky subscription responses to product,
  currency, price, billing period, and usage components; avoid payment details.
- Catch errors per independent request so one failing endpoint does not discard
  successful results from others.
- Do not interpret HTTP 200 alone as success; inspect the service result too.

## R2 operations through existing OAuth

Distinguish **bucket management**, **object access**, and **API-token management**.
A failure on `/user/tokens` or `/accounts/{account_id}/tokens` does not establish
that R2 operations are unavailable. Conversely, successful bucket creation alone
is not proof of object access. Probe the actual operation needed.

### Prefer the existing MCP connection before requiring S3 keys

Load the official `cloudflare` skill and its `references/r2/README.md` and relevant
references through `local-skills`. Then discover R2 endpoints with the registered
`cloudflare-api` Code Mode `search` tool. Inspect only the needed methods,
parameters, request bodies, and response fields; returning every R2 schema can
truncate the MCP response.

The following REST routes were found in the MCP's OpenAPI specification:

| Operation | Method and path |
| --- | --- |
| List/create buckets | `GET` / `POST /accounts/{account_id}/r2/buckets` |
| Inspect bucket | `GET /accounts/{account_id}/r2/buckets/{bucket_name}` |
| List objects | `GET /accounts/{account_id}/r2/buckets/{bucket_name}/objects` |
| Read/write/delete one object | `GET` / `PUT` / `DELETE /accounts/{account_id}/r2/buckets/{bucket_name}/objects/{object_key}` |

Use `cloudflare.request` through Executor with the existing authorized OAuth
connection. Supply the explicit account and bucket, and encode the object key as
one path parameter. For listings, inspect `per_page`, `prefix`, and `cursor` in the
current schema, bound each response, and follow pagination when completeness is
required. Do not use the collection DELETE route for a single-object cleanup.

**Verified in this environment:** the existing OAuth connection successfully
listed objects, uploaded a small non-secret JSON object, read it back with an exact
content match, deleted it, and confirmed its absence. API-token-management access
still returned error `9109` after reauthorization. These are separate capabilities.
This demonstrates an R2 object-access route without issuing new S3 credentials,
deploying a Worker, making the bucket public, or adding a daemon. It does not prove
that every connection, jurisdiction, payload size, or future deployment supports
this route; rediscover and verify before relying on it.

### JSON/binary handling and verification

- The discovered GET schema returns the object body, not necessarily a Cloudflare
  JSON API envelope. PUT accepts a raw object body. Inspect the current MCP
  `execute` schema for `body`, `contentType`, and `rawBody` handling; do not assume
  arbitrary binary data survives the MCP's response parser.
- The successful small-object probe used `contentType: "application/json"` and an
  application document shaped as
  `{"success":true,"result":{"marker":"<random non-secret value>"},"errors":[],"messages":[]}`.
  On GET, the marker was available as `response.result.marker`. The stored
  `success` field is application data, **not independent evidence of API success**;
  check HTTP status and the exact returned content as well.
- For encrypted settings, a JSON envelope containing base64-encoded ciphertext is
  a candidate transport, not a tested full synchronization implementation. Encrypt
  locally before upload; base64 is not encryption. Never place plaintext secrets
  in tool arguments, object probes, logs, or MCP responses.
- Check MCP input/output limits independently of the REST upload limit. Test
  realistic encrypted payload sizes, truncation detection, integrity, pagination,
  and any chunking protocol before enabling synchronization. Do not treat failed
  requests or truncated results as an absent object or empty bucket.
- Start investigations with bounded reads. Run write probes only when authorized:
  use a unique key in the intended bucket, non-secret data, exact read-back checks,
  and cleanup in a `finally` path. Delete only the object created by that probe,
  verify absence, and report cleanup failures without hiding the original error.
- Preserve bucket privacy and unrelated lifecycle rules. Distinguish a successful
  transport probe from encryption setup, synchronization enablement, and actual
  second-device restoration.

### Other R2 routes and authentication limits

- The installed `cloudflare-bindings` catalog exposed bucket create/get/list/delete
  tools, not object read/write tools, during verification. Inspect its current
  catalog rather than assuming it has every R2 operation.
- S3-compatible clients remain a valid alternative when S3 credentials are already
  available or specifically needed. They are **not a prerequisite for all R2
  access**. An existing S3-only implementation needs a transport change to use
  Executor/MCP REST; finding this route does not automatically change that code.
- Temporary S3 credentials derive from an existing parent R2 API token. They do
  not remove that prerequisite or expand the parent's permissions. Inspect the
  current Temporary Credentials API before proposing it as a solution.
- Check granted permissions and the official MCP scope catalog linked below before
  proposing reauthorization. At verification, the published OAuth catalog had no
  API-token-management scope; `access-service-token.read/write` concerns Zero Trust
  service tokens, not general API-token management. Do not promise that selecting
  Full access or repeating OAuth will add an unsupported permission.
- Only require a credential handoff or Dashboard token issuance when the chosen
  route actually needs it and available authorized routes have been evaluated.
  Keep existing working connections intact and never extract their credentials
  from Executor storage to bypass the integration.

References:

- Official R2 skill: `cloudflare/references/r2/` via `local-skills`.
- Current REST schemas: registered `cloudflare-api` Code Mode `search`.
- R2 S3 token authentication: https://developers.cloudflare.com/r2/api/tokens/
- Temporary credentials: https://developers.cloudflare.com/r2/api/s3/temporary-credentials/
- Official API MCP: https://github.com/cloudflare/mcp

## Billing and authentication troubleshooting

Treat these as separate states: catalog registration, MCP connection, tool listing,
resource access, billing access, and successful monetary calculation.

An OAuth connection that can list accounts may still lack Billing permissions.
Do not assume reauthorization adds an unsupported scope. Check the granted scopes
and current official MCP scope catalog before proposing reauthorization:

- https://github.com/cloudflare/mcp/blob/main/src/auth/scopes.ts
- https://github.com/cloudflare/mcp/blob/main/src/auth/derived-oauth-scopes.json
- https://github.com/cloudflare/mcp#option-2-api-token

OAuth discovery metadata may omit `scopes_supported`. An empty discovery result
is not proof that no scopes exist. Public repository code is evidence of published
behavior, not proof of the exact deployed version.

The official API MCP also supports API-token Bearer authentication. If OAuth cannot
provide billing access, reuse or add a separate token-authenticated connection to
that same MCP endpoint; a separate REST integration is not inherently necessary.
Keep existing working OAuth connections intact.

Billing access needs the currently documented Billing Read permission. MCP identity
resolution can additionally require user and account reads. Check current official
requirements before asking a human to edit the token:

- User-owned tokens: identity probes can call `/user` and `/accounts`.
- Account-owned tokens: the official README requires Account Resources Read to
  discover the account, which must resolve to exactly one account.
- The official README currently disallows Client IP Address Filtering for MCP tokens.

Scope the token to the requested account and minimum necessary read permissions.
Do not request broad edit permissions just to diagnose a connection failure.
If Executor only returns a generic transport error, report the cause as unconfirmed;
do not assert that permissions are definitely the problem.

## Billing investigation workflow

1. Determine the current date/time and explicit account. Separate calendar-month
   usage from the account's actual subscription billing cycle.
2. For current-cycle charges, prioritize the **Billable Usage** API, not
   `billing/usage` or invoice history. Discover and inspect these endpoints through
   Code Mode `search` before using them:
   - `GET /accounts/{account_id}/billable-usage/info`: check `covered` and subscription
     metadata, including `billing_cycle_anchor_timestamp`.
   - `GET /accounts/{account_id}/billable-usage`: omit dates to request the current
     billing period. An explicit `from`/`to` range must include the subscription's
     billing-cycle anchor day; otherwise the API may return no usage.
   - `GET /accounts/{account_id}/subscriptions`: obtain fixed recurring prices and
     current period dates separately. These are not invoice totals.
   The similarly named `/billable/usage` (v2) is a different, restricted API. Its
   documented cost fields may be absent. Inspect the current specification rather
   than assuming the newest version provides monetary values.
3. Aggregate Billable Usage **inside MCP Code Mode before returning the result**;
   full daily records can exceed MCP output limits. Follow the aggregation and
   reporting checklist below. If a history endpoint fails, preserve successful
   Billable Usage results rather than declaring billing inaccessible.
4. Use `cloudflare-graphql` for measured usage. Discover dataset names, then use the
   actual discovered type-details tool (names may differ from its description).
   Inspect sums, units, filters, retention, sampling, and limits before querying.
   Follow query-tool restrictions and request approval of the exact query when required.
5. Use `cloudflare-docs` for current unit prices, allowances, rounding, and billing
   rules before calculating. Check other usage-priced products, not just Workers/R2.
6. Report confirmed fixed fees, measured variable usage, estimated variable cost,
   and unavailable products separately. Include the time range and uncertainty.

Critical pitfalls:

- `billing/usage` without explicit metrics can default to only `streamMinutesViewed`.
  Zero Stream minutes does not mean the account has no billable usage.
- Subscription `price: 0` for R2 does not imply free total usage.
- Usage component `value: 1` is not evidence of one measured request or GB.
- Monthly fixed fees are not the final invoice, and may exclude taxes/credits.
- Workers CPU time can be in microseconds; convert to the pricing unit explicitly.
- R2 maximum storage is not GB-month consumption; obtain the needed storage-time
  aggregation and operation classes before computing a storage bill.
- Respect retention, sampling, and truncated results. Empty rows do not prove zero
  charges unless coverage and dataset semantics have been verified.

### Billable Usage aggregation and validation

Official reference:
https://developers.cloudflare.com/billing/manage/billable-usage/

The dashboard uses the invoice-generating data source, but shows **usage overage
charges only**, not fixed subscriptions. In an open billing cycle the returned
charges are accrued amounts, not a final invoice or a forecast of future usage.

Verified in this environment: the token-authenticated `cloudflare-billing-mcp`
connection successfully returned `billable-usage/info` and `billable-usage` when
invoice history failed. This is evidence for trying this route, not a guarantee
that every account or future API version supports it.

For a bounded returned summary:

1. Check API `success`, account identity, array shape, and any pagination metadata.
   Fetch remaining pages if the current API requires them. Never total a truncated
   text response; retrieve and aggregate the complete API result within Code Mode.
2. Report row count, missing/non-numeric cost count, distinct `BillingPeriodStart`,
   subscription IDs, and currencies. Do not silently treat missing costs as zero.
3. Sum daily **`ContractedCost`** once per record, grouping by currency, billing
   period, subscription, and `ServiceFamilyName`. Optionally include `ServiceName`
   for finer investigation. Do not combine different currencies into one total.
4. **Never sum `CumulatedContractedCost` across daily rows**: it is a running total
   and would double-count earlier charges. Do not add `BilledCost`, `EffectiveCost`,
   or `ListCost` to `ContractedCost`; these are alternative cost representations.
5. Return the earliest `ChargePeriodStart`, latest `ChargePeriodEnd`, and distinct
   daily intervals. Compare intervals with the requested period to detect gaps;
   a maximum timestamp alone does not prove complete coverage for every product.
6. Treat interval ends as exclusive where documented: an end of September 12 at
   00:00 UTC means data through September 11, not all of September 12. State the
   actual data cutoff and reporting lag rather than claiming real-time costs.
7. Keep full precision during aggregation; round monetary values only for display.
   Report zero-cost families separately from missing/unavailable families.
8. If adding fixed subscription fees, verify frequency, billing cycle, currency,
   cancellation/trial state, and whether the fees apply to the next invoice.
   Avoid counting subscription fees already included in another returned amount.
   State the assumption when the invoice itself cannot be verified.
9. Present accrued usage + applicable fixed fees as a **provisional subtotal**, not
   an issued invoice or guaranteed next-month bill. Future usage, taxes, credits,
   discounts, prorations, and unreported charges may change the final amount.

Do not hardcode previously observed dollar amounts, account IDs, subscription IDs,
or dates into future investigations. Fetch fresh data for the user's account.
When the account is not covered or the endpoint fails, use the documented Dashboard
path **Manage Account → Billing → Billable Usage**, or report a limited usage-based
estimate with its assumptions. Do not silently substitute a zero total.

## Completion report

Report succinctly:

- Skill path and which official skills were reused or installed.
- Executor integrations/connections actually verified, with no secret values.
- Any pending human consent, unavailable services, or untested assumptions.
- For billing, confirmed amounts versus estimates and remaining gaps.

Do not output the official all-complete banner while any required setup or live
verification remains incomplete.
