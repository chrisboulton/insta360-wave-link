# Insta360 InSight meeting and transcript API research

Research date: 2026-10-04. This note separates published developer documentation from private web-client interfaces. No account credentials or personal meeting contents were accessed for the published-documentation research.

## Published documentation

### Finding

No released public REST endpoint for listing a user's InSight meetings or retrieving their transcripts was identified in the official documentation inspected. This is a documentation finding, not proof that a private or partner API does not exist. The official Open Platform API overview explicitly labels its capabilities **Planning Stage**, covers account/content/store domains, and says detailed endpoints are coming later. [Official API overview](https://open.insta360.com/en/api/)

### Planned public API, not a usable InSight contract

The specification gives the planned base URL `https://open.insta360.com/api/v1`, HTTPS JSON requests, and a response envelope with `code`, `message`, and `data`. Its planning notice means these conventions are not evidence of a released meeting API. [Official API specification](https://open.insta360.com/en/api/specification)

The authentication page describes planned OAuth 2.0 Authorization Code authorization and Bearer access tokens. It lists these scopes:

| Scope | Documented capability |
| --- | --- |
| `account:read` | Basic account profile |
| `content:read` | Media list and metadata |
| `content:write` | Media upload/edit |
| `store:read` | Products, pricing and orders |
| `store:write` | Orders and purchases |

No meeting/transcript-specific scope is listed. The page gives a conceptual application-registration and token-exchange flow, but no concrete authorization or token endpoint URLs. It also carries the planning notice. Therefore `content:read` cannot currently be assumed to authorize InSight meeting transcripts. [Official authentication documentation](https://open.insta360.com/en/api/authentication)

The endpoint catalog publishes only these **planned path prefixes**, rather than request methods, complete resource endpoints, parameter schemas, pagination, or transcript response formats:

| Domain | Planned prefix |
| --- | --- |
| Common | `/api/v1/common/` |
| Account | `/api/v1/account/` |
| Content | `/api/v1/content/` |
| Store | `/api/v1/store/` |

The catalog says concrete definitions will follow service development; it does not document an InSight business domain. [Official API endpoint catalog](https://open.insta360.com/en/api/endpoints)

### Confirmed product capabilities

The Wave support manual identifies the service as **Insta360 InSight**, at `https://insight.insta360.com`. After binding an account, enabling Cloud Sync uploads completed recordings over the device's internet connection. InSight can display cloud recordings and generate transcriptions and summaries. These are documented product features, not documented third-party API operations. [Wave InSight tutorial](https://onlinemanual.insta360.com/wave/en-us/camera/advanced-use/use-insight-ai)

Insta360 also advertises downloading audio, notes and transcripts through InSight's user interface. This establishes an export fallback if a reliable API integration is unavailable. [Official InSight product page](https://store.insta360.com/gb/product/insight)

### Enterprise API is a separate lead

The **Wave For Business** manual advertises integration of recording/transcription files with enterprise systems (printed page 1), local recording file uploads through a device API (page 9), and remote power/recording/audio controls (page 26). It directs developers to `devops.conf@insta360.com` for complete interface documentation. Its specification lists meeting recording/file management among API capabilities (page 31). This is a partner-support lead, without published endpoint or authentication details; consumer Wave compatibility and access to an existing consumer InSight account's cloud history are unconfirmed. [Official Wave For Business manual](https://wassets.insta360.com/common/864b93fff69e43a5a3358f90af2dec95/Wave%20For%20Business_User%20Manual_EN_v1.0.pdf)

### Integration implication

Public OAuth documentation is insufficient to implement this today. A consumer integration would need verified read-only web-client requests and session handling, or additional official partner documentation. Any web-client endpoints discovered separately should be marked private/undocumented and should not be presented as the planned Open Platform API. No partner-contact message was sent.

## Current private web API

### Finding and evidence

The consumer InSight web client has usable, authenticated meeting-library APIs.
Live read-only requests using the existing official controller's account session
successfully listed cloud meetings, read recording details, retrieved non-empty
transcript text with sentence timestamps and speaker labels, and read AI summary
components. Offset pagination was verified by comparing overlapping list pages
in memory. No meeting titles, identifiers, transcript text, credentials, or signed
playback URLs are included in this note or saved as research artifacts.

Primary implementation sources inspected:

- [InSight web entry point](https://insight.insta360.com/file).
- [Current main client bundle](https://insight.insta360.com/assets/index-Td2HPuvX.js): API wrappers, authentication cookie, regional routing, transcript models.
- [Meeting library/detail client bundle](https://insight.insta360.com/assets/index.page-B2FAZkkY.js): list request parameters, response pagination, transcript rendering and exports.
- [Folder client bundle](https://insight.insta360.com/assets/InboxIcon-CdXyMeaO.js): folder-list API.

The bundle filenames are deployment-specific. The main bundle's SHA-256 at the
research date was `3dfeb6dbfefb93b98a8e7b0645949742113382496ab8039da74568b5f272bae4`;
the meeting detail bundle's was `8f4aaec81049a3a094f62fec2fe610505f3901773568b6f194f7042e86dd47d3`.
These sources establish the current web client's behavior, not a supported
third-party API contract.

### Service and authentication

The web client routes summary requests to these production service bases:

```text
https://service-met-va.insta360.com/summary-service
https://service-met-hz.insta360.com/summary-service
https://service-met-fra.insta360.com/summary-service
https://service-met-hk.insta360.com/summary-service
```

Use the authenticated account's region; do not try every region with the token.
The existing app already reads the controller's selected `EndPoint` and
`PcToken` through `WaveCloudSession`. Live verification used that selected
production endpoint and passed the existing token only as an `X-User-Token`
header. The web client obtains its token from the `insta_access_token` cookie
and sends the same header. Do not confuse this account token with the USB
binding key or firmware `X-Device-Token`. [Main client bundle](https://insight.insta360.com/assets/index-Td2HPuvX.js),
[existing account-session implementation](../macos/Sources/WaveDesktop/WaveCloudClient.swift).

JSON replies use a `code`/`data` envelope; the tested successful replies had
`code: 0`. Live research sent `Accept: application/json`, `X-User-Token`, and
`X-Language: en-US`; the JSON list request also used `Content-Type: application/json`.
Requests did not follow redirects with the account token.

### Endpoint inventory

Paths below are relative to the regional `/summary-service` base. Verification
means a successful read-only response was received from the current account;
it does not imply every possible account or processing state was tested.

| Method | Path | Purpose | Evidence |
| --- | --- | --- | --- |
| POST | `/summary/selectSummaryListByIndex` | Paginated recording library | Client source + live list/pagination |
| GET | `/summary/selectRecentSummaryList` | Recent recording subset | Client source |
| GET | `/summary/selectBaseInfoDetail` | Recording metadata and `playAudioUrl` | Client source + live detail |
| GET | `/summary/selectTranscriptionDetail` | Full transcript paragraphs and sentences | Client source + live non-empty transcript |
| GET | `/summary/selectAllSegmentDetail` | Speaker segment boundaries and indices | Client source + live segments |
| GET | `/summary/selectAllSpeaker` | Speaker list | Client source + live list |
| GET | `/summary/getTaskInfos` | ASR, summary, template, actions, speaker and chapter results | Client source + live results |
| GET | `/summary/getTaskStatus` | Processing status | Client source |
| GET | `/summary/selectChapterSummary` | Chapter summaries | Client source |
| GET | `/summary/getExportAudioUrl` | Audio export/download URL | Client source; export URL not requested live |
| POST | `/summary/exportTranslateResult` | Transcript file export | Client source; export not requested live |
| POST | `/summary/exportTaskInfos` | Summary file export | Client source; export not requested live |
| GET | `/fileCategory/selectCategoryList` | Folder list | Client source; not requested live |

The main bundle's `pt` helper issues GET requests with query parameters; its
`nt` helper issues POST requests with JSON bodies. The export implementation
uses POST with parameters in the URL query and authenticated download headers.
Transcript/summary export parameters include `meetingId`, `transcriptionId`,
and `exportFormat`; summary export also passes `language`. Exact supported file
formats still require verification. [API wrappers](https://insight.insta360.com/assets/index-Td2HPuvX.js),
[export implementation](https://insight.insta360.com/assets/index.page-B2FAZkkY.js).

### Recording list and pagination

Verified list body:

```json
{
  "index": 0,
  "limitSize": 30,
  "sortFlag": "create_time_desc"
}
```

`data.summaryDataList` is the page and `data.totalCount` is the matching total.
`index` is an item offset, not a page number. The client advances it by the
number of entries already loaded. Omitting `categoryId` lists the overall
library; folder views add `categoryId`. The client also uses `clientSource` for
special views. Known sort values are `create_time_desc`, `create_time_asc`,
`duration_desc`, and `duration_asc`. Sizes 5 and 30 were verified; an attempted
size 60 was rejected, so a larger maximum must not be assumed. [List implementation](https://insight.insta360.com/assets/index.page-B2FAZkkY.js),
[sort constants](https://insight.insta360.com/assets/index-Td2HPuvX.js).

Meeting metadata includes `meetingId`, `transcriptionId`, `summaryName`,
`duration`, `formatDateTime`, `createTime`, `deviceType`, `status`,
`isSummaryOpen`, `categoryId`, `isExample`, `templateId`, and `templateType`.
The successful pagination check confirmed that an offset-5 page matched items
5–9 of an offset-0 larger page. A library integration can traverse the full
cloud list with bounded pages until it reaches the total or an empty page.

### Transcript shape

Verified request:

```text
GET /summary/selectTranscriptionDetail?meetingId=<meeting-id>
X-User-Token: <account-token>
```

The response has `data.transcriptionId` and
`data.summaryDialogueRecordList`. Despite that field name, the latter contains
the spoken transcript, not just assistant chat history. Its paragraph objects
include `index`, `speaker`, `startTime`, `endTime`, `avatarColor`, `title`,
`sentenceBOList`, and `screenShotBOList`. Sentences include `content`,
`startTime`, `endTime`, `index`, `speaker`, and `updated`.

Live inspection confirmed non-empty sentence `content` and timestamps on every
sentence of the inspected completed transcript. The UI converts transcript
timestamps to playback seconds by dividing by 1000: these are milliseconds
relative to the recording, not Unix timestamps. [Transcript model](https://insight.insta360.com/assets/index-Td2HPuvX.js),
[playback/highlighting implementation](https://insight.insta360.com/assets/index.page-B2FAZkkY.js).

`selectAllSegmentDetail` is a lightweight speaker/timing list, not the full text
endpoint. Newly recorded or unprocessed meetings can return a successful reply
with an empty transcript array. An empty array must not be presented as a
failed login or a permanently missing transcript.

`getTaskInfos` returns component objects with `taskType`, `status`, `result`,
and sometimes `resultId`. Observed components include `ASR`, `ABSTRACT`,
`TEMPLATE`, `ONE_PAGE`, `AUDIO_PLAY`, `TODO`, `SPEAKER_INFO`, and chapter-summary
results. `ABSTRACT.result` contains `summaryContent`, `summaryName`, and
`suggestionProblem`; action/template/speaker results have their own shapes.

### Scope and next implementation

This supports a native **Cloud library** with recording history, transcript
search/display, speaker/timestamp navigation, summaries/action items, and audio
playback. The existing account-session and regional HTTP helpers can be reused.
No app feature was implemented in this research task.

This is the cloud-synced library. Recordings kept only on the physical Wave
still need a separate device file-transfer path; the cloud API cannot retrieve
files that were never uploaded. Cloud Sync is documented separately in the
[Wave InSight tutorial](https://onlinemanual.insta360.com/wave/en-us/camera/advanced-use/use-insight-ai).

Implementation should distinguish local Wave state from cloud processing state,
load a bounded metadata page before fetching transcripts on demand, handle
session expiry without USB write retries, and keep tokens in Keychain. An
independent sign-in/session-refresh flow is still needed to fully replace the
official controller's existing login. Authentication errors, transcript-only
recordings, pending processing, deleted meetings, folder filtering, and export
formats need coverage before claiming full replacement parity.

## Native library implementation notes

The public web client redirects to
`https://www.insta360.com/account/login-standalone` with `platform=summary` and
`redirect_url=https://insight.insta360.com/file?lang=en-us`. The native app uses
that official page in a nonpersistent WKWebView, reads only the InSight account
cookie after returning to the exact InSight HTTPS origin, and requires the user
to confirm the account region before sending API requests. This sign-in flow
was inspected in public JavaScript, but interactive login has not been tested.

The detail bundle renders recording `duration` with a `milliseconds` argument;
both cloud duration and transcript timestamps are milliseconds. Native duration
is converted to seconds for the existing duration formatter. TODO results use
an `answer` array of `checked` and `answerContent` records; speaker mentions use
`<at ... speakerName="..."/>` tags, rendered as plain names in local exports.
AUDIO_PLAY URLs are excluded from archived task content. Optional playback
downloads refresh `playAudioUrl` from recording details; real audio downloads
remain unverified.
