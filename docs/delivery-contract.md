# Core and operator delivery contract

Core and operator contributors use this contract to preserve registration
ownership across replacement and recovery. A retained volume contains runtime
state. It does not prove which Kubernetes installation may use that state.

This document resolves the implementation rules for
[Core issue #8](https://github.com/rhaf-ai/rhaf-portal-core/issues/8).
The rules below define the Core-side implementation contract. Operator
maintainers must confirm alignment at the integration gate below. This document
does not claim that the current runtime implements the planned lifecycle or
adoption operations, or that both repositories have approved this contract.

## Registration identity and ownership

The public `registrationId` identifies one registration lifetime. It remains
stable through replacement, restart, and metadata-only adoption. Deletion
followed by recreation produces a new identity, even when the name, address,
and namespace base remain the same. Use the read-only identity from the
[RHAF API](api-contracts.md#registration-identity-decision). Do not parse internal
labels or derive an identity from a name.

The operator records that identity with the owning `RHAFPortalCore` UID and the
source `RHAFMCPServer` UID. Record the source namespace and name for lookup and
diagnostics. UIDs determine ownership; names alone do not. Core distinguishes
operator-managed registrations from explicit registrations. An explicit
registration has no Kubernetes source owner and cannot enter operator adoption.

Core keeps ownership metadata and namespace reservations with the registration.
The immutable namespace base reserves both `<base>-all` and `<base>-progressive`
until durable cleanup completes. An empty or unowned namespace still conflicts
with a new reservation. User labels cannot set or overwrite lifecycle metadata.

The public identity does not act as a general conditional-write token. Normal
replacement and discovery publication require internal configuration and
lifetime guards. In particular, an unchanged address does not make an old
discovery result safe after credential or configuration changes.

## Retained-data recovery

| Situation | Required behavior |
| --- | --- |
| Restart with the same Core and source UIDs | Load retained state and resume incomplete lifecycle work. Keep registration identities and ownership. No adoption is necessary. |
| Replacement Core UID with surviving source UIDs | Stop the previous runtime. Require explicit administrative adoption before the new installation changes the retained registrations. |
| Restore in another namespace or cluster | Treat recreated Core and source resources as new identities. Do not match them by name or automatically adopt them. Automated UID remapping is outside the MVP. |

A new Core resource must not inherit retained ownership merely because it can
mount the old PVC. The operator must stop the old workload before it releases
the installation reservation. A data-directory lock alone does not prove that
an old runtime on another volume or cluster has stopped.

### Administrative adoption operation

[Core #27](https://github.com/rhaf-ai/rhaf-portal-core/issues/27) must implement a
supported metadata operation with these semantics. It must not reuse forward
replacement, repeat upstream discovery, or require snapshot-file edits.
The operation is not an implemented route in the current OpenAPI contract.

For each registration, the request identifies the expected `registrationId`,
previous Core UID, surviving source UID, and target Core UID. Include the source
namespace and name so the operator can verify the live source resource. The
operator must verify that the previous runtime has stopped and cannot restart
during transfer. A trusted administrator must provide this verification for
recovery that the operator cannot observe. A caller-supplied boolean alone does
not establish that condition.

Core must compare the expected identity and previous ownership with durable
state under its lifecycle commit guard. Reject missing registrations, explicit
registrations, mismatched owners, recreated sources, and registrations with
incomplete deletion. Do not change a rejected registration or its reservations.
The operator must retain exclusive installation ownership throughout transfer.

For a valid request, change only the Core ownership metadata. Preserve the
registration identity, source identity, desired configuration, credentials
references, namespace reservations, catalog, and lifecycle state. An unavailable
upstream must not prevent adoption. Do not change availability as a side effect
of the metadata operation.

Commit each registration transfer atomically and acknowledge success only after
durable persistence. A retry with the same identity, source, and target owner
must return the completed result without another transfer. A conflicting retry
must fail. After a persistence failure or lost response, read durable ownership
before retrying. Report per-registration outcomes so an interrupted multi-record
recovery can resume without claiming batch atomicity. Crash recovery must expose
either the previous owner or the committed target owner for each registration.

Core #27 must add the request, response, failure statuses, and examples to the
generated schema before the operator calls the operation. The operator must
reach it through the restricted management boundary. Registration identity and
knowledge of a previous UID do not authorize adoption.

### Required recovery material

The administrator must retain a consistent recovery set:

- The PVC data and its storage identity, backup point, and Core data format.
- The previous Core manifest and UID, target Core UID, installation namespace,
  and installation reservation with its ownership metadata.
- Each endpoint declaration, its UID, namespace, name, desired configuration,
  and recorded registration identity and ownership status.
- Configuration objects and all referenced credential bindings and Secrets.
  Back up secret values through the administrator's secure backup process.
- The durable OpenShift Lightspeed (OLS) ownership ledger, including previous
  values and ownership needed to restore or remove only RHAF-managed changes.
- The Core, operator, UI, and upstream revisions used to write the retained
  state, plus evidence that the previous runtime cannot restart.

Cluster administrators own backup execution and secret restoration. A PVC
restore alone does not restore Kubernetes UIDs, endpoint declarations,
installation ownership, credential inputs, or the OLS ledger.

The operator README's data-and-Core-manifest namespace-move procedure covers
workload relocation only. It does not establish managed-registration recovery.
[Core #28](https://github.com/rhaf-ai/rhaf-portal-core/issues/28) and
[operator #11](https://github.com/rhaf-ai/rhaf-portal-operator/issues/11) must
replace that guidance with an exercised runbook before advertising recovery.
Keep cross-cluster migration automation outside the MVP.

## Implementation boundaries

Use this table when task descriptions appear to conflict. Preserve the approved
public registration identity while keeping internal mutation guards separate.
Do not use private upstream handlers or direct snapshot changes to bridge a
missing supported interface.

| Area | Current capability or required work |
| --- | --- |
| Current Core runtime | Embedded Wanaku, shared MCP and management state, persistent registry and audit stores, exclusive data-directory locking, virtual namespaces, and audit-mode governance. The README describes the runnable configuration and controlled MCP check. |
| Current RHAF API | Generated lifecycle and catalog schema. Declarations do not install route handlers. Adoption has no route yet. |
| Downstream implementation | Core #10–#17 supply management, lifecycle reservations, mutation guards, catalog, durable deletion, and restart recovery. Core #27 supplies adoption. The operator retains declarations, inputs, PVC data, and its OLS ledger under operator #11. |
| Required upstream extensions | Core #11 supplies shared guarded publication for every discovery writer. Core #19 supplies the WASM tool-handler contract. Core #29 supplies healthy-forward refresh. Core #30 proves integration and pins the resulting supported dependency. |
| Deferred scope | Automatic endpoint discovery, a full upstream admin UI, lossless audit history, new backup infrastructure, automated cross-cluster migration, and management authentication implementation. Restrict management access with the documented deployment boundary. |

A conventional MCP path alone does not satisfy progressive delivery. Keep the
progressive host and dispatch requirements in scope. Do not treat planned
upstream extensions as capabilities of the currently pinned dependency.

## Quality and release gates

Use successive evidence layers. A higher layer does not replace a lower one.

| Layer | Required evidence |
| --- | --- |
| Native checks | Rust build, tests, formatting, Clippy, lint inheritance, and schema drift checks. Operator, UI, and fixture repositories retain their native deterministic suites. No model provider is necessary. |
| Runtime integration | Controlled real MCP forwarding with independent fixture observations, persistence and restart checks, lifecycle races, stale-publication rejection, adoption conflicts, and crash recovery at commit boundaries. Mocks alone cannot qualify these guarantees. |
| Deployment and clients | OpenShift operator lifecycle, retained-data recovery, UI/browser access, developer clients, credentials, progressive dispatch, and workload limits against the exact candidate. Real OLS qualification is mandatory for OLS-enabled support. An explicitly OLS-disabled candidate does not require it. |
| Human release decision | A named human reviews exact-candidate results, unresolved defects, accepted limitations, and compatibility claims in Core #33. AI assessments, skipped tests, and optional evidence cannot count as passes. |

Record source revisions, image digests, configuration, platform/client versions,
commands, and retained results with each qualification run. Re-run affected
gates after dependency changes. Do not release with an unresolved defect that
violates ownership, persistence, dispatch, or credential guarantees.

### Initial compatibility targets

The initial platform targets are OpenShift 4.22 and OpenShift 5.0. The 5.0
target does not establish compatibility with every OpenShift 5.x release.
For optional OLS-enabled deployments, qualify OpenShift Lightspeed 2.0 on each
target. OLS has its own version lifecycle. The
[Red Hat OpenShift Operator life cycle policy](https://access.redhat.com/support/policy/updates/openshift_operators),
checked on 2026-11-24, lists OLS 2.0 support for OpenShift 4.22 and 5.0.
That upstream support statement does not establish RHAF compatibility.
Record exact platform patches, OLS ClusterServiceVersion (CSV), and image digests
at candidate qualification. Keep OLS-disabled support separate from OLS-enabled
support.

Bob, Jolokia endpoints, and other forwards use a protocol and capability
contract. They must comply with the MCP versions, transports, and capabilities
that the candidate Core supports. Do not add product-version allowlists or
arbitrary product-version gates. Record the concrete product and client versions
in qualification evidence so another contributor can reproduce the check.
Protocol compliance does not replace real forwarding, client, credential, or
dispatch checks.

The portal browser targets are the stable Chrome, Firefox, Edge, and Safari
releases available at candidate qualification. Use standard interoperable web
features and perform the necessary compatibility checks across those browsers.
Record exact browser builds and operating systems with the results.

These are qualification targets, not completed support claims.
[Core #31](https://github.com/rhaf-ai/rhaf-portal-core/issues/31) must publish the
tested matrix, Core-supported MCP versions and transports, and explicit evidence
gaps before release. Do not count an untested target as a pass.

### Open decisions and accountable gates

The compatibility targets above still require candidate-specific evidence.
Human release ownership still requires explicit acceptance. The following gates
keep pending work accountable; they do not imply approval by the named maintainer.

| Decision or deliverable | Accountable owner | Gate |
| --- | --- | --- |
| Adoption schema and guarded durable implementation | Core maintainers; coordination: @orpiske | Core #27 and operator #11 must agree on the generated contract before operator integration. Core #28 must exercise it before recovery support. |
| Shared publication, WASM hosting, and healthy refresh | Core maintainers; coordination: @orpiske | Core #30 must record upstream regression evidence and the consumed pin before dependent feature qualification. |
| Compatibility qualification | Product maintainers; coordination: @orpiske | Qualify the initial targets above. Core #31 must publish exact tested configurations, protocol coverage, and explicit evidence gaps before support claims or release. |
| Human release owner | Project maintainer @orpiske must name the owner and obtain acceptance | Record the person's name and acceptance in [Core #33](https://github.com/rhaf-ai/rhaf-portal-core/issues/33) before candidate assessment. That person must record the final release decision. |
| Cross-repository recovery guidance and evidence | Core and operator maintainers; coordination: @orpiske | Core #28 and operator #11 must reconcile the relocation instructions and retain recovery evidence before documentation claims complete recovery. |

Continue dependent implementation against these rules. Keep integration or
release gates closed until their accountable owners record the required evidence
and decisions.
