# Insta360 Wave outbound HTTP protocol

This document describes the Wave's outbound cloud, log-upload, and OTA traffic recovered from the extracted firmware. It is a static analysis result; endpoint selection, field presence, and request ordering may differ at runtime.

## Source files

- `extracted/Wave/rootfs/usr/lib/libcloud_proto_https.so`
- `extracted/Wave/rootfs/usr/lib/libins_entry.so`
- `extracted/Wave/rootfs/etc/ssl/certs/ca-certificates.crt`

The main cloud client is unstripped and contains DWARF debug information.

## Architecture

The Wave has three outbound HTTP subsystems:

1. **Cloud protocol client** (`libcloud_proto_https.so`) — meeting, transcription, binding, auth, vocabulary, and speaker requests.
2. **Chunked file/log uploader** (`libins_entry.so`) — region-selected firmware-log upload with signed control requests and direct object-storage chunk upload.
3. **OTA updater** (`libins_entry.so`) — shells out to `curl`; metadata download disables certificate verification with `-k`.

Normal cloud requests use libcurl 7.68.0 with mbedTLS. The direct file upload uses a separate raw-PUT helper.

## Base URLs

### Production summary service

Regional selection uses country/data-storage configuration:

```text
https://openapi-hz.insta360.com/summary-service
https://openapi-va.insta360.com/summary-service
https://openapi-fra.insta360.com/summary-service
```

`get_data_storage_by_country` maps a country to one of the `hz`, `va`, or `fra` bases.

### Fallback and test bases

```text
https://api.insta360.com
https://openapi-hk-test.insta360.com/summary-service
https://openapi-psp-test.insta360.com/summary
https://service-met-sit.insta360.com/summary-service
https://service-met-hk-test.insta360.com/summary-service
```

The last base is also the built-in default for the chunked log uploader.

### OTA

```text
https://openapi.insta360.com/app/v1/common/upgrade/psp_firmware
https://openapi.insta360.com/app/v1/common/upgrade/psp_model

https://openapi-test.insta360.com/app/v1/common/upgrade/psp_firmware
https://openapi-test.insta360.com/app/v1/common/upgrade/psp_model
```

### Untraced plain-HTTP hosts

```text
http://47.115.40.252
http://ezy.lcyq.vip:97
```

Both strings occur in the firmware, but their call paths have not yet been traced.

## Common request format

### Standard headers

JSON GET and POST requests use:

```http
Content-Type: application/json
User-Agent: PSP_DEVICE
Accept: */*
Cache-Control: no-cache
Accept-Encoding: gzip, deflate, br
Connection: keep-alive
```

Authenticated requests add:

```http
X-Device-Token: <device-token>
```

Signed requests add:

```http
X-Device-Id: <device-id>
X-Timestamp: <timestamp>
X-Nonce: <nonce>
X-Signature: <signature>
```

Direct upload PUT requests include:

```http
Content-Type: <server-provided content type>
Content-Length: <file size>
Content-MD5: <md5>            # optional on the meeting-file path
X-Check:IGNORE                # meeting-file path
```

Chunk uploads use:

```http
Content-Type: application/octet-stream
Content-Length: <chunk size>
```

### Response envelope

Normal JSON responses are parsed as:

```json
{
  "code": 0,
  "data": {}
}
```

`code` is extracted and passed to a response callback. HTTP statuses from `200` through `299` are treated as successful. Other statuses and libcurl errors are translated into an internal error enum.

## Authentication

### Device secret registration

Endpoint:

```text
POST /device/secret/register
```

Recovered request fields:

```json
{
  "deviceId": "...",
  "firmwareVersion": "...",
  "nonce": "..."
}
```

Recovered response fields:

```json
{
  "data": {
    "deviceSecret": "...",
    "salt": "..."
  }
}
```

The returned secret and salt are decrypted by AES helpers in the cloud library and persisted.

### Token refresh

Endpoint:

```text
POST /device/secret/token/refresh
```

Recovered request fields:

```json
{
  "deviceId": "...",
  "timestamp": 1234567890,
  "nonce": "...",
  "oldToken": "..."
}
```

The exact signature construction was recovered from:

- `cloud::CloudProtoHttps::generateTokenRefreshSignature`
- `get_signature_headers`

It is:

```text
message   = deviceId + timestamp + nonce
key       = deviceSecret + salt
signature = lowercase_hex(HMAC-SHA256(key, message))
```

The result is sent as `X-Signature`.

## Endpoint inventory

### Meeting and transcription

| Method | Endpoint | Purpose |
|---|---|---|
| `POST` | `/device/summary/asr/startSummaryWithTranscription` | Initialize meeting/cloud transcription |
| `POST` | `/device/summary/asr/task` | ASR notice or task operation |
| `POST` | `/device/summary/qrcode` | Request meeting QR code |
| `POST` | `/upload/dotTask` | Meeting dot notice |
| `POST` | `/upload/meetingInfo` | Meeting info notice |
| `GET` | `/summary/speaker/syncSpeaker` | Sync speaker names |

Meeting initialization request:

```json
{
  "localMeetingId": "...",
  "deviceType": "PSP",
  "deviceId": "...",
  "language": "en_US",
  "formatDatetime": "YYYY-MM-DD HH:MM:SS",
  "timezone": "...",
  "isTranscriptOnly": true,
  "meeting_type": 0,
  "isSummaryOpen": true,
  "startTime": 1234567890,
  "userCloudTranscription": true
}
```

Recovered language values include:

```text
zh_CN, en_US, de_DE, fr_FR, ja_JP, zh_CN_HK, zh_TW, zh_CN_YUE
```

### Meeting file upload

| Method | Endpoint | Purpose |
|---|---|---|
| `POST` | `/upload/uploadInit` | Prepare one meeting file upload |
| `POST` | `/upload/getUploadSignUrl` | Alternative signed URL preparation |
| `POST` | `/upload/reportFileUploadStatus` | Report file/chunk status |
| `POST` | `/upload/completeUpload` | Complete upload task |
| `POST` | `/upload/uploadTaskFinishNotice` | Final completion notice |
| `PUT` | `<returned fileUrl>` | Direct raw object-storage upload |

Upload preparation request:

```json
{
  "meetingId": "...",
  "fileType": 1,
  "fileMd5": "...",
  "index": 0,
  "supportContentType": true
}
```

Preparation response:

```json
{
  "data": {
    "fileUrl": "...",
    "fileObjectName": "...",
    "fileHeader": {
      "value": "..."
    },
    "contentType": "..."
  }
}
```

Status request:

```json
{
  "cloud_meeting_id": "...",
  "fileType": 1,
  "fileMd5": "...",
  "index": 0,
  "isMeetingEnd": true,
  "isAllFileUploadSuccess": true
}
```

### Device binding

#### Get bound user information

Endpoint:

```http
GET /deviceBinding/getUserInfo
```

The C++ method requires three local input fields:

```json
{
  "base_url": "https://openapi-hz.insta360.com/summary-service",
  "device_token": "...",
  "device_id": "..."
}
```

`base_url` selects the service root. `device_token` is converted into the
authentication header:

```http
X-Device-Token: <device-token>
```

`device_id` is appended to the final URL as a query parameter:

```http
GET <base_url>/deviceBinding/getUserInfo?device_id=<device-id>
```

The C ABI wrapper (`cloud_device_binding_get_user_info`) receives the same
three values as strings, constructs the JSON object shown above, and calls the
C++ method. The `timezone` string is accepted by the C ABI but is not added to
the request; it is unrelated to the HTTP call.

This method does not extract a user-info object from the response. If the
transport succeeds and the HTTP status is exactly `200`, the complete response
body is copied verbatim to the caller. The body is then parsed only to extract
the standard response `code` for the asynchronous response callback. As a
result, the user-info fields are not recoverable from this library alone; the
consumer of the C ABI determines the schema.

The expected envelope is therefore:

```json
{
  "code": 0,
  "data": {
    "...": "..."
  }
}
```

Any `data` fields present in a live response can be recorded by intercepting
the device request or by finding the downstream consumer.

#### Get membership URL

Endpoint:

```http
GET /deviceBinding/membership
```

Local input and query construction match `getUserInfo`:

```json
{
  "base_url": "...",
  "device_token": "...",
  "device_id": "..."
}
```

Final request:

```http
GET <base_url>/deviceBinding/membership?device_id=<device-id>
X-Device-Token: <device-token>
```

The response is parsed as:

```json
{
  "code": 0,
  "data": "https://..."
}
```

`data` must be a JSON string. If `code` is nonzero, `data` is missing, or
`data` is not a string, the method returns failure. On success, the URL is
written to the output string.

#### Sync web unbind information

Endpoint:

```http
GET /deviceBinding/syncWebUnbindInfo
```

The same three local inputs are required, and the request uses the same
`X-Device-Token` header. Like `getUserInfo`, this method returns the raw
response body after a successful transport and HTTP `200`; it does not decode
a binding-user object in this library. The downstream consumer must be
inspected to recover its response fields.

#### Create Wave binding task

Endpoint:

```http
POST /wave/binding/task/create
```

The local credential object requires:

```json
{
  "base_url": "...",
  "deviceId": "...",
  "timestamp": 1234567890,
  "nonce": "...",
  "deviceSecret": "...",
  "salt": "..."
}
```

The request body is the empty JSON object:

```json
{}
```

The signature and headers are the same as token refresh:

```text
message   = deviceId + timestamp + nonce
key       = deviceSecret + salt
signature = lowercase_hex(HMAC-SHA256(key, message))
```

```http
POST <base_url>/wave/binding/task/create
X-Device-Id: <device-id>
X-Timestamp: <timestamp>
X-Nonce: <nonce>
X-Signature: <signature>
Content-Type: application/json
```

Response:

```json
{
  "code": 0,
  "data": {
    "qrcodeUrl": "https://...",
    "taskId": "...",
    "expireRemainingSeconds": 300
  }
}
```

`qrcodeUrl` and `taskId` are strings, and `expireRemainingSeconds` is a signed
integer. The method returns failure if `code` is nonzero, any of these fields
is missing, or the HTTP status is not exactly `200`.

#### Poll Wave binding task

Endpoint:

```http
GET /wave/binding/task/status
```

The local input object requires `taskId` plus the same six credential fields
as task creation:

```json
{
  "base_url": "...",
  "taskId": "...",
  "deviceId": "...",
  "timestamp": 1234567890,
  "nonce": "...",
  "deviceSecret": "...",
  "salt": "..."
}
```

`taskId` is percent-encoded with libcurl and appended as a query parameter:

```http
GET <base_url>/wave/binding/task/status?taskId=<percent-encoded-task-id>
X-Device-Id: <device-id>
X-Timestamp: <timestamp>
X-Nonce: <nonce>
X-Signature: <signature>
```

The response has the standard envelope. `data.status` is required and is a
string. The parser recognizes three status-specific payloads:

```json
{
  "code": 0,
  "data": {
    "status": "USER_CONFIRMED",
    "deviceInfo": {
      "...": "..."
    },
    "taskInfo": {
      "...": "..."
    },
    "confirmInfo": {
      "userEndPoint": "...",
      "dataRegion": 1,
      "userNickName": "..."
    }
  }
}
```

For `DEVICE_CONFIRMED`, the parser uses the same `confirmInfo` object. For
`USER_CONFIRMED`, `deviceInfo` and `taskInfo` are also read when present.

For `TASK_EXPIRED`, the parser reads `data.expireInfo`:

```json
{
  "code": 0,
  "data": {
    "status": "TASK_EXPIRED",
    "expireInfo": {
      "...": "..."
    }
  }
}
```

The library logs but does not reject unrecognized top-level fields inside
`data`. The exact nested fields of `deviceInfo`, `taskInfo`, `confirmInfo`,
and `expireInfo` are not decoded by this library and remain to be recovered
from a live response or downstream consumer.

#### Confirm Wave binding task

Endpoint:

```http
POST /wave/binding/task/confirm
```

The local input object requires `taskId`, `dataRegion`, and the same six
credential fields as task creation:

```json
{
  "base_url": "...",
  "taskId": "...",
  "dataRegion": 1,
  "deviceId": "...",
  "timestamp": 1234567890,
  "nonce": "...",
  "deviceSecret": "...",
  "salt": "..."
}
```

Request:

```json
{
  "taskId": "...",
  "dataRegion": 1
}
```

The same HMAC headers are sent. Response:

```json
{
  "code": 0,
  "requestId": "...",
  "msg": "...",
  "data": {
    "deviceToken": "...",
    "bindKey": "...",
    "endpoint": "...",
    "userId": 123456
  }
}
```

`deviceToken`, `bindKey`, and `endpoint` are required strings. `userId` is a
signed integer and defaults to zero when absent. `requestId` and `msg` are
read from the top-level response when present. The method returns failure if
`code` is nonzero, a required `data` field is missing, or the HTTP status is
not exactly `200`.

#### Cancel Wave binding task

Endpoint:

```http
POST /wave/binding/task/cancel
```

The local input object requires `taskId` and the same six credential fields
as task creation:

```json
{
  "base_url": "...",
  "taskId": "...",
  "deviceId": "...",
  "timestamp": 1234567890,
  "nonce": "...",
  "deviceSecret": "...",
  "salt": "..."
}
```

`taskId` is percent-encoded and included both in the URL and in the empty
JSON request body:

```http
POST <base_url>/wave/binding/task/cancel?taskId=<percent-encoded-task-id>
X-Device-Id: <device-id>
X-Timestamp: <timestamp>
X-Nonce: <nonce>
X-Signature: <signature>
Content-Type: application/json
```

```json
{}
```

Response:

```json
{
  "code": 0,
  "data": true,
  "requestId": "..."
}
```

`data` must be a boolean and must be `true`. The method also reads `msg` and
`error` from the response for failure logging. It returns failure if `code`
is nonzero, `data` is missing or false, or the HTTP status is not exactly
`200`.

The firmware contains both `uploadInit` and `getUploadSignUrl`; the exact rule for choosing one has not yet been recovered.

### Vocabulary

| Method | Endpoint | Purpose |
|---|---|---|
| `GET` | `/vocabulary/checkVersion` | Get cloud hotword version |
| `GET` | `/vocabulary/syncWords` | Get hotword list |

### Device binding and user information

| Method | Endpoint | Purpose |
|---|---|---|
| `GET` | `/deviceBinding/getUserInfo` | Get bound user |
| `GET` | `/deviceBinding/membership` | Get membership URL |
| `GET` | `/deviceBinding/syncWebUnbindInfo` | Sync web-initiated unbind |
| `POST` | `/wave/binding/task/create` | Create Wave binding task |
| `GET` | `/wave/binding/task/status` | Poll binding task |
| `POST` | `/wave/binding/task/confirm` | Confirm binding |
| `POST` | `/wave/binding/task/cancel` | Cancel binding |
| `POST` | `/summary/service/device/password/recover/sendcode` | Send password reset code |
| `POST` | `/summary/service/device/password/recover/verifycode` | Verify reset code |

Binding task creation returns `taskId`, `qrcodeUrl`, and
`expireRemainingSeconds`.

### Abnormal meetings and miscellaneous

| Method | Endpoint | Purpose |
|---|---|---|
| `GET` | `/device/abnormalMeeting/list` | List abnormal meetings |
| `POST` | `/device/abnormalMeeting/reportComplete` | Report abnormal-meeting handling complete |
| `GET` | `/collect/getCollectUrl` | Get collect/feedback URL |
| `GET` | `/upload/getCurTimeStamp` | Time synchronization |

## Chunked log upload

Control endpoints:

```text
POST /firmware/log/upload/prepare
GET  /firmware/log/upload/chunk
POST /firmware/log/upload/chunk/complete
POST /firmware/log/upload/chunk/error
POST /firmware/log/upload/complete
GET  /firmware/log/upload/progress
POST /firmware/log/upload/cancel
```

### Prepare

```json
{
  "fileName": "...",
  "fileSize": 123456,
  "md5": "...",
  "firmwareVersion": "..."
}
```

Response:

```json
{
  "data": {
    "chunkSize": 1048576,
    "expiresAt": 1234567890,
    "totalChunks": 12,
    "storageType": "...",
    "taskId": "...",
    "uploadId": "..."
  }
}
```

### Chunk sequence

1. Build the chunk control URL from `taskId`, `uploadId`, and `chunkIndex`.
2. `GET` that URL.
3. Parse `data.uploadUrl`.
4. `PUT` raw chunk bytes to `uploadUrl`.
5. Capture the response `ETag`.
6. `POST /firmware/log/upload/chunk/complete` with `taskId`, `uploadId`, `chunkIndex`, and `etag`.

Error reporting uses `errorCode` and `errorMessage`.

Progress response includes:

```text
businessType
completedChunks
status
statusDesc
progressPercent
createTime
updateTime
expireTime
chunks
```

Each item in `chunks` includes:

```text
chunkNumber
completeTime
retryCount
```

The log uploader uses the same HMAC authentication headers through `get_signature_headers`.

## OTA protocol

### Metadata request

The updater invokes:

```sh
curl -s -vk \
  -H "X-Device-Serial: <serial>" \
  -H "X-Device-Version: <version>" \
  <metadata-url> \
  -o <output-file>
```

`-k` disables certificate verification for metadata.

Metadata shape:

```json
{
  "data": {
    "appVersion": {
      "version": "...",
      "download_url": "...",
      "file_md5": "...",
      "descriptions": "...",
      "forced": 0
    }
  }
}
```

### Package download

The package downloader uses:

```sh
curl -C - -o <package> <download-url> \
  --progress-bar \
  --write-out '%{size_download}/%{size_total}' \
  > /tmp/curl_progress_<task> 2>&1 \
  && echo done > /tmp/swu.done
```

The package command does not visibly include `-k`. The MD5 is validated; failed packages are deleted and retried. A stalled download is restarted after approximately 30 seconds without progress.

## TLS and interception

No application-level certificate pinning was found.

- Normal cloud/log clients leave libcurl's certificate-chain and hostname verification enabled.
- A MITM proxy is therefore practical if the Wave trusts the proxy CA or the leaf certificate chains to an existing trusted root.
- OTA metadata can be intercepted even with an untrusted certificate because it uses `curl -k`.
- OTA package download likely rejects an untrusted MITM certificate.

## Practical capture setup

1. Run `mitmproxy` or an equivalent TLS interception proxy.
2. Redirect the target hostnames at DNS or the network gateway.
3. For normal cloud/log traffic, install a trusted CA on the Wave root filesystem or present a certificate chaining to an existing root.
4. For OTA metadata, an untrusted certificate is sufficient.
5. Record the exact route and field variants; the firmware contains fallback and test paths that may not all be used in production.

## Open questions

- Which conditions select `/upload/uploadInit` versus `/upload/getUploadSignUrl`.
- Exact completion order after `reportFileUploadStatus`, `completeUpload`, and `uploadTaskFinishNotice`.
- Numeric mapping of every HTTP status to the firmware's internal error enum.
- Purpose of the two plain-HTTP hosts.
- Live endpoint behavior and any server-required fields not present in the client strings.
