module Api exposing
    ( Activity
    , AttemptOutcome
    , AutofixAttempt
    , AutofixQueue
    , Cycle
    , DailyOutcome
    , Error(..)
    , FailingPackage
    , IndexEntry
    , LabelCount
    , Log
    , Outcomes
    , Package
    , PackagePhase
    , Phase
    , PhaseStat
    , Repo
    , Session
    , SessionDetail
    , Source
    , SourceTypeCount
    , StatusCounts
    , activity
    , andMap
    , attemptOutcome
    , autofixAttempt
    , autofixQueue
    , cycle
    , dailyOutcome
    , expectValue
    , failingPackage
    , indexEntry
    , labelCount
    , log
    , nullable
    , outcomes
    , package
    , packagePhase
    , phase
    , phaseStat
    , repo
    , required
    , session
    , sessionDetail
    , source
    , sourceTypeCount
    , statusCounts
    )

{-| Row types served by the JSON API (`ekapkgs-update-web/src/queries/`)
and their decoders. Field names follow the Rust structs; `Option` fields
are `Maybe`.
-}

import Http
import Json.Decode as D exposing (Decoder)



-- REQUESTS


{-| Why a request produced no data.
-}
type Error
    = NotFound String
    | Failed String


{-| Expect a JSON body; the page decoder runs later (in `Main`). Error
bodies (`{"error": "...", "status": 404}`) keep their message.
-}
expectValue : (Result Error D.Value -> msg) -> Http.Expect msg
expectValue toMsg =
    Http.expectStringResponse toMsg <|
        \response ->
            case response of
                Http.GoodStatus_ _ body ->
                    D.decodeString D.value body
                        |> Result.mapError (\_ -> Failed "The server sent a response the dashboard could not read.")

                Http.BadStatus_ meta body ->
                    let
                        message =
                            D.decodeString (D.field "error" D.string) body
                                |> Result.withDefault ("The server answered with status " ++ String.fromInt meta.statusCode ++ ".")
                    in
                    if meta.statusCode == 404 then
                        Err (NotFound message)

                    else
                        Err (Failed message)

                Http.Timeout_ ->
                    Err (Failed "The request timed out.")

                Http.NetworkError_ ->
                    Err (Failed "Could not reach the dashboard server.")

                Http.BadUrl_ url ->
                    Err (Failed ("Bad URL: " ++ url))



-- PIPELINE HELPERS


andMap : Decoder a -> Decoder (a -> b) -> Decoder b
andMap =
    D.map2 (|>)


required : String -> Decoder a -> Decoder (a -> b) -> Decoder b
required key decoder =
    andMap (D.field key decoder)


{-| A field that is always present but may be `null`.
-}
nullable : String -> Decoder a -> Decoder (Maybe a -> b) -> Decoder b
nullable key decoder =
    andMap (D.field key (D.nullable decoder))


str : String -> Decoder (String -> b) -> Decoder b
str key =
    required key D.string


int : String -> Decoder (Int -> b) -> Decoder b
int key =
    required key D.int


bool : String -> Decoder (Bool -> b) -> Decoder b
bool key =
    required key D.bool


mstr : String -> Decoder (Maybe String -> b) -> Decoder b
mstr key =
    nullable key D.string


mint : String -> Decoder (Maybe Int -> b) -> Decoder b
mint key =
    nullable key D.int



-- PACKAGES


type alias Package =
    { repoName : String
    , attrPath : String
    , status : String
    , currentVersion : Maybe String
    , latestUpstreamVersion : Maybe String
    , proposedVersion : Maybe String
    , baselineVersion : Maybe String
    , failedVersion : Maybe String
    , failureCount : Int
    , lastError : Maybe String
    , lastSessionId : Maybe String
    , lastChecked : Maybe String
    , lastAttempted : Maybe String
    , nextAttempt : Maybe String
    , prUrl : Maybe String
    , prNumber : Maybe Int
    , rebuildCount : Maybe Int
    , cveResolvedCount : Int
    , cveIntroducedCount : Int
    , pname : Maybe String
    , description : Maybe String
    , homepage : Maybe String
    , upstreamType : Maybe String
    , upstreamKey : Maybe String
    , upstreamInstance : Maybe String
    , semverStrategy : Maybe String
    , isBacklog : Bool
    }


package : Decoder Package
package =
    D.succeed Package
        |> str "repo_name"
        |> str "attr_path"
        |> str "status"
        |> mstr "current_version"
        |> mstr "latest_upstream_version"
        |> mstr "proposed_version"
        |> mstr "baseline_version"
        |> mstr "failed_version"
        |> int "failure_count"
        |> mstr "last_error"
        |> mstr "last_session_id"
        |> mstr "last_checked"
        |> mstr "last_attempted"
        |> mstr "next_attempt"
        |> mstr "pr_url"
        |> mint "pr_number"
        |> mint "rebuild_count"
        |> int "cve_resolved_count"
        |> int "cve_introduced_count"
        |> mstr "pname"
        |> mstr "description"
        |> mstr "homepage"
        |> mstr "upstream_type"
        |> mstr "upstream_key"
        |> mstr "upstream_instance"
        |> mstr "semver_strategy"
        |> bool "is_backlog"


type alias StatusCounts =
    { total : Int
    , pending : Int
    , upToDate : Int
    , outdated : Int
    , backlog : Int
    , proposed : Int
    , failed : Int
    , upstreamError : Int
    , openPrs : Int
    }


statusCounts : Decoder StatusCounts
statusCounts =
    D.succeed StatusCounts
        |> int "total"
        |> int "pending"
        |> int "up_to_date"
        |> int "outdated"
        |> int "backlog"
        |> int "proposed"
        |> int "failed"
        |> int "upstream_error"
        |> int "open_prs"


type alias IndexEntry =
    { repoName : String
    , attrPath : String
    , upstreamType : String
    , upstreamKey : String
    , instance : Maybe String
    , currentVersion : Maybe String
    , srcUrl : Maybe String
    , indexedAt : String
    , pname : Maybe String
    , semverStrategy : Maybe String
    , versionRegex : Maybe String
    , includePrereleases : Bool
    }


indexEntry : Decoder IndexEntry
indexEntry =
    D.succeed IndexEntry
        |> str "repo_name"
        |> str "attr_path"
        |> str "upstream_type"
        |> str "upstream_key"
        |> mstr "instance"
        |> mstr "current_version"
        |> mstr "src_url"
        |> str "indexed_at"
        |> mstr "pname"
        |> mstr "semver_strategy"
        |> mstr "version_regex"
        |> bool "include_prereleases"


type alias AutofixQueue =
    { id : Int
    , repoName : String
    , attrPath : String
    , sessionId : String
    , errorType : String
    , failedPhase : String
    , status : String
    , priority : Int
    , attempts : Int
    , maxAttempts : Int
    , createdAt : String
    , updatedAt : String
    , fixedAt : Maybe String
    , artifactsPath : Maybe String
    }


autofixQueue : Decoder AutofixQueue
autofixQueue =
    D.succeed AutofixQueue
        |> int "id"
        |> str "repo_name"
        |> str "attr_path"
        |> str "session_id"
        |> str "error_type"
        |> str "failed_phase"
        |> str "status"
        |> int "priority"
        |> int "attempts"
        |> int "max_attempts"
        |> str "created_at"
        |> str "updated_at"
        |> mstr "fixed_at"
        |> mstr "artifacts_path"


type alias AutofixAttempt =
    { id : Int
    , queueId : Int
    , repoName : String
    , attemptNumber : Int
    , startedAt : String
    , completedAt : Maybe String
    , promptTokens : Maybe Int
    , completionTokens : Maybe Int
    , changesApplied : Bool
    , buildSuccess : Maybe Bool
    , status : String
    , errorMessage : Maybe String
    }


autofixAttempt : Decoder AutofixAttempt
autofixAttempt =
    D.succeed AutofixAttempt
        |> int "id"
        |> int "queue_id"
        |> str "repo_name"
        |> int "attempt_number"
        |> str "started_at"
        |> mstr "completed_at"
        |> mint "prompt_tokens"
        |> mint "completion_tokens"
        |> bool "changes_applied"
        |> nullable "build_success" D.bool
        |> str "status"
        |> mstr "error_message"



-- SESSIONS AND PHASES


type alias Session =
    { id : String
    , mode : String
    , status : String
    , startedAt : String
    , completedAt : Maybe String
    , updatedAt : Maybe String
    , attempted : Int
    , succeeded : Int
    , failed : Int
    , skipped : Int
    , evaluated : Int
    , backoff : Int
    }


session : Decoder Session
session =
    D.succeed Session
        |> str "id"
        |> str "mode"
        |> str "status"
        |> str "started_at"
        |> mstr "completed_at"
        |> mstr "updated_at"
        |> int "packages_attempted"
        |> int "packages_succeeded"
        |> int "packages_failed"
        |> int "packages_skipped"
        |> int "packages_evaluated"
        |> int "packages_backoff"


type alias SessionDetail =
    { session : Session
    , configJson : Maybe String
    }


sessionDetail : Decoder SessionDetail
sessionDetail =
    D.succeed SessionDetail
        |> andMap session
        |> mstr "config_json"


type alias Phase =
    { id : Int
    , sessionId : String
    , repoName : String
    , attrPath : String
    , phase : String
    , startedAt : String
    , completedAt : Maybe String
    , durationMs : Maybe Int
    , status : String
    , errorType : Maybe String
    , errorDetails : Maybe String
    , artifactsPath : Maybe String
    }


phase : Decoder Phase
phase =
    D.succeed Phase
        |> int "id"
        |> str "session_id"
        |> str "repo_name"
        |> str "attr_path"
        |> str "phase"
        |> str "started_at"
        |> mstr "completed_at"
        |> mint "duration_ms"
        |> str "status"
        |> mstr "error_type"
        |> mstr "error_details"
        |> mstr "artifacts_path"


{-| A phase with its session (package history).
-}
type alias PackagePhase =
    { phase : Phase
    , sessionMode : Maybe String
    , sessionStatus : Maybe String
    , sessionStartedAt : Maybe String
    }


packagePhase : Decoder PackagePhase
packagePhase =
    D.succeed PackagePhase
        |> andMap phase
        |> mstr "session_mode"
        |> mstr "session_status"
        |> mstr "session_started_at"


{-| A recent phase with its session and package state (dashboard feed).
-}
type alias Activity =
    { phase : Phase
    , sessionMode : Maybe String
    , sessionStatus : Maybe String
    , prUrl : Maybe String
    , prNumber : Maybe Int
    , currentVersion : Maybe String
    , proposedVersion : Maybe String
    , failedVersion : Maybe String
    }


activity : Decoder Activity
activity =
    D.succeed Activity
        |> andMap phase
        |> mstr "session_mode"
        |> mstr "session_status"
        |> mstr "pr_url"
        |> mint "pr_number"
        |> mstr "current_version"
        |> mstr "proposed_version"
        |> mstr "failed_version"


{-| Outcomes of finished sessions in a time window.
-}
type alias Outcomes =
    { sessions : Int
    , attempted : Int
    , succeeded : Int
    , failed : Int
    , skipped : Int
    }


outcomes : Decoder Outcomes
outcomes =
    D.succeed Outcomes
        |> int "sessions"
        |> int "attempted"
        |> int "succeeded"
        |> int "failed"
        |> int "skipped"



-- FAILURE LOGS


type alias Log =
    { drvPath : String
    , repoName : String
    , attrPath : String
    , timestamp : String
    , status : String
    , oldVersion : Maybe String
    , newVersion : Maybe String
    , sessionId : Maybe String
    , phase : Maybe String
    , errorType : Maybe String

    -- The last `LOG_TAIL_CHARS` (6,000) characters of the log.
    , logTail : String
    , logLen : Int
    }


log : Decoder Log
log =
    D.succeed Log
        |> str "drv_path"
        |> str "repo_name"
        |> str "attr_path"
        |> str "timestamp"
        |> str "status"
        |> mstr "old_version"
        |> mstr "new_version"
        |> mstr "session_id"
        |> mstr "phase"
        |> mstr "error_type"
        |> str "log_tail"
        |> int "log_len"



-- WATCH


type alias Repo =
    { name : String
    , upstream : Maybe String
    , path : String
    , entrypoint : String
    , baseBranch : Maybe String
    , forkRemote : Maybe String
    , headCommit : Maybe String
    , headSubject : Maybe String
    , syncStatus : Maybe String
    , syncError : Maybe String
    , lastSyncedAt : Maybe String
    , indexStatus : Maybe String
    , indexError : Maybe String
    , lastIndexedAt : Maybe String
    , indexDurationMs : Maybe Int
    , evaluatedCount : Maybe Int
    , indexedCount : Maybe Int
    , indexSkipped : Maybe Int
    , indexErrors : Maybe Int
    , bootstrappedAt : Maybe String
    , createdAt : String
    , updatedAt : String
    , backlogCount : Int
    , indexRows : Int
    }


repo : Decoder Repo
repo =
    D.succeed Repo
        |> str "name"
        |> mstr "upstream"
        |> str "path"
        |> str "entrypoint"
        |> mstr "base_branch"
        |> mstr "fork_remote"
        |> mstr "head_commit"
        |> mstr "head_subject"
        |> mstr "sync_status"
        |> mstr "sync_error"
        |> mstr "last_synced_at"
        |> mstr "index_status"
        |> mstr "index_error"
        |> mstr "last_indexed_at"
        |> mint "index_duration_ms"
        |> mint "evaluated_count"
        |> mint "indexed_count"
        |> mint "index_skipped"
        |> mint "index_errors"
        |> mstr "bootstrapped_at"
        |> str "created_at"
        |> str "updated_at"
        |> int "backlog_count"
        |> int "index_rows"


type alias Cycle =
    { id : Int
    , kind : String
    , repoName : Maybe String
    , startedAt : String
    , completedAt : Maybe String
    , durationMs : Maybe Int
    , status : String
    , bootstrap : Bool
    , sourcesPolled : Int
    , sourcesFailed : Int
    , packagesObserved : Int
    , newReleases : Int
    , dispatched : Int
    , updated : Int
    , failed : Int
    , sessionId : Maybe String
    , error : Maybe String
    }


cycle : Decoder Cycle
cycle =
    D.succeed Cycle
        |> int "id"
        |> str "kind"
        |> mstr "repo_name"
        |> str "started_at"
        |> mstr "completed_at"
        |> mint "duration_ms"
        |> str "status"
        |> bool "bootstrap"
        |> int "sources_polled"
        |> int "sources_failed"
        |> int "packages_observed"
        |> int "new_releases"
        |> int "dispatched"
        |> int "updated"
        |> int "failed"
        |> mstr "session_id"
        |> mstr "error"


type alias Source =
    { upstreamType : String
    , upstreamKey : String
    , instance : String
    , latestVersion : Maybe String
    , latestTag : Maybe String
    , releaseCount : Maybe Int
    , latestChangedAt : Maybe String
    , firstSeenAt : String
    , lastPolledAt : Maybe String
    , lastSuccessAt : Maybe String
    , lastError : Maybe String
    , consecutiveFailures : Int
    , packageCount : Int
    }


source : Decoder Source
source =
    D.succeed Source
        |> str "upstream_type"
        |> str "upstream_key"
        |> str "instance"
        |> mstr "latest_version"
        |> mstr "latest_tag"
        |> mint "release_count"
        |> mstr "latest_changed_at"
        |> str "first_seen_at"
        |> mstr "last_polled_at"
        |> mstr "last_success_at"
        |> mstr "last_error"
        |> int "consecutive_failures"
        |> int "package_count"


type alias SourceTypeCount =
    { upstreamType : String
    , total : Int
    , failing : Int
    , neverPolled : Int
    }


sourceTypeCount : Decoder SourceTypeCount
sourceTypeCount =
    D.succeed SourceTypeCount
        |> str "upstream_type"
        |> int "total"
        |> int "failing"
        |> int "never_polled"



-- ANALYTICS


type alias LabelCount =
    { label : String
    , count : Int
    }


labelCount : Decoder LabelCount
labelCount =
    D.succeed LabelCount
        |> str "label"
        |> int "count"


type alias PhaseStat =
    { phase : String
    , total : Int
    , succeeded : Int
    , failed : Int
    , skipped : Int
    , running : Int
    , avgMs : Maybe Float
    , maxMs : Maybe Int
    }


phaseStat : Decoder PhaseStat
phaseStat =
    D.succeed PhaseStat
        |> str "phase"
        |> int "total"
        |> int "succeeded"
        |> int "failed"
        |> int "skipped"
        |> int "running"
        |> nullable "avg_ms" D.float
        |> mint "max_ms"


{-| Outcomes per day; `day` is `YYYY-MM-DD` (UTC).
-}
type alias DailyOutcome =
    { day : String
    , sessions : Int
    , succeeded : Int
    , failed : Int
    }


dailyOutcome : Decoder DailyOutcome
dailyOutcome =
    D.succeed DailyOutcome
        |> str "day"
        |> int "sessions"
        |> int "succeeded"
        |> int "failed"


type alias AttemptOutcome =
    { status : String
    , count : Int
    , promptTokens : Int
    , completionTokens : Int
    }


attemptOutcome : Decoder AttemptOutcome
attemptOutcome =
    D.succeed AttemptOutcome
        |> str "status"
        |> int "count"
        |> int "prompt_tokens"
        |> int "completion_tokens"


type alias FailingPackage =
    { repoName : String
    , attrPath : String
    , failures : Int
    , lastFailure : String
    , consecutive : Maybe Int
    , lastErrorType : Maybe String
    }


failingPackage : Decoder FailingPackage
failingPackage =
    D.succeed FailingPackage
        |> str "repo_name"
        |> str "attr_path"
        |> int "failures"
        |> str "last_failure"
        |> mint "consecutive"
        |> mstr "last_error_type"
