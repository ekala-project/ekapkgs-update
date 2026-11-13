module Page.Session.ItemTest exposing (suite)

import Api
import Expect
import Json.Decode as D
import Page.Session.Item as Item
import Table exposing (cases)
import Test exposing (Test, describe, test)
import Time


{-| 2026-11-11T12:00:00Z
-}
now : Time.Posix
now =
    Time.millisToPosix 1794398400000


session : Api.Session
session =
    { id = "1b9d6bcd-bbfd-4b2d-9b5d-ab8dfbbd4bed"
    , mode = "run"
    , status = "completed"
    , startedAt = "2026-11-11T10:00:00Z"
    , completedAt = Just "2026-11-11T11:30:00Z"
    , updatedAt = Just "2026-11-11T11:00:00Z"
    , attempted = 10
    , succeeded = 3
    , failed = 2
    , skipped = 1
    , evaluated = 1200
    , backoff = 40
    }


running : Api.Session
running =
    { session | status = "running", completedAt = Nothing }


suite : Test
suite =
    describe "Page.Session.Item"
        [ test "a completed session" <|
            \_ ->
                let
                    i =
                        Item.item now session
                in
                Expect.equal
                    { isRunning = False
                    , stale = False
                    , heartbeat = "2026-11-11T11:00:00Z"
                    , duration = "1h 30m"
                    , badge = { label = "completed", tone = "good" }
                    , inFlight = 4
                    , finished = 6
                    , progressText = "6 of 10 finished"
                    , successRate = "60.0%"
                    }
                    { isRunning = i.isRunning
                    , stale = i.stale
                    , heartbeat = i.heartbeat
                    , duration = i.duration
                    , badge = i.badge
                    , inFlight = i.inFlight
                    , finished = i.finished
                    , progressText = i.progressText
                    , successRate = i.successRate
                    }
        , cases "duration (status, completedAt, updatedAt)"
            (\( status, completedAt, updatedAt ) ->
                (Item.item now { session | status = status, completedAt = completedAt, updatedAt = updatedAt }).duration
            )
            [ ( ( "completed", Just "2026-11-11T11:30:00Z", Just "2026-11-11T11:00:00Z" ), "1h 30m" )
            , ( ( "interrupted", Nothing, Just "2026-11-11T11:00:00Z" ), "1h 0m" )
            , ( ( "failed", Nothing, Nothing ), "0ms" )
            , ( ( "running", Nothing, Just "2026-11-11T11:00:00Z" ), "2h 0m" )
            , ( ( "running", Just "2026-11-11T10:30:00Z", Nothing ), "2h 0m" )
            , ( ( "completed", Just "garbage", Nothing ), "—" )
            ]
        , cases "stale (status, updatedAt)"
            (\( status, updatedAt ) ->
                let
                    i =
                        Item.item now { session | status = status, completedAt = Nothing, updatedAt = updatedAt }
                in
                ( i.stale, i.badge )
            )
            [ ( ( "running", Just "2026-11-11T11:59:00Z" ), ( False, { label = "running", tone = "info" } ) )
            , ( ( "running", Just "2026-11-11T06:00:00Z" ), ( False, { label = "running", tone = "info" } ) )
            , ( ( "running", Just "2026-11-11T05:59:59Z" ), ( True, { label = "running · stale?", tone = "serious" } ) )
            , ( ( "running", Just "not a time" ), ( False, { label = "running", tone = "info" } ) )
            , ( ( "completed", Just "2026-11-01T00:00:00Z" ), ( False, { label = "completed", tone = "good" } ) )
            , ( ( "interrupted", Just "2026-11-01T00:00:00Z" ), ( False, { label = "interrupted", tone = "serious" } ) )
            ]
        , test "a running session without a heartbeat uses its start time" <|
            \_ ->
                let
                    i =
                        Item.item now { running | startedAt = "2026-11-10T12:00:00Z", updatedAt = Nothing }
                in
                Expect.equal ( "2026-11-10T12:00:00Z", True ) ( i.heartbeat, i.stale )
        , cases "counters (attempted, succeeded, failed, skipped)"
            (\( attempted, ( succeeded, failed, skipped ) ) ->
                let
                    i =
                        Item.item now { session | attempted = attempted, succeeded = succeeded, failed = failed, skipped = skipped }
                in
                ( i.inFlight, i.progressText, i.successRate )
            )
            [ ( ( 10, ( 3, 2, 1 ) ), ( 4, "6 of 10 finished", "60.0%" ) )
            , ( ( 0, ( 0, 0, 0 ) ), ( 0, "0 of 0 finished", "—" ) )
            , ( ( 5, ( 0, 0, 5 ) ), ( 0, "5 of 5 finished", "—" ) )
            , ( ( 1, ( 3, 0, 0 ) ), ( 0, "3 of 1 finished", "100.0%" ) )
            , ( ( 12345, ( 1000, 234, 11 ) ), ( 11100, "1,245 of 12,345 finished", "81.0%" ) )
            ]
        , cases "Api.session decoder"
            (D.decodeString Api.session >> Result.toMaybe)
            [ ( sessionJson "\"2026-11-11T11:30:00Z\"", Just session )
            , ( sessionJson "null", Just { session | completedAt = Nothing } )
            , ( "{}", Nothing )
            ]
        ]


sessionJson : String -> String
sessionJson completedAt =
    """{"id":"1b9d6bcd-bbfd-4b2d-9b5d-ab8dfbbd4bed","mode":"run","status":"completed",
"started_at":"2026-11-11T10:00:00Z","completed_at":"""
        ++ completedAt
        ++ """,
"updated_at":"2026-11-11T11:00:00Z","packages_attempted":10,"packages_succeeded":3,
"packages_failed":2,"packages_skipped":1,"packages_evaluated":1200,"packages_backoff":40}"""
