module Page.Session.Item exposing (Item, item, progressBar)

{-| A session row prepared for display (shared by the session list and
the session page).
-}

import Api
import Format
import Html exposing (Html)
import Time
import View


{-| A running session whose heartbeat is older than this is flagged stale.
-}
staleAfterMs : Int
staleAfterMs =
    6 * 3600 * 1000


type alias Item =
    { session : Api.Session
    , isRunning : Bool
    , stale : Bool
    , heartbeat : String
    , duration : String
    , badge : Format.Badge
    , inFlight : Int
    , finished : Int
    , progressText : String
    , successRate : String
    }


item : Time.Posix -> Api.Session -> Item
item now s =
    let
        isRunning =
            s.status == "running"

        heartbeat =
            Maybe.withDefault s.startedAt s.updatedAt

        stale =
            isRunning
                && (Format.parseTs heartbeat
                        |> Maybe.map (\hb -> Time.posixToMillis now - Time.posixToMillis hb > staleAfterMs)
                        |> Maybe.withDefault False
                   )

        end =
            if isRunning then
                Nothing

            else
                Just (s.completedAt |> orElse s.updatedAt |> Maybe.withDefault s.startedAt)

        inFlight =
            max 0 (s.attempted - s.succeeded - s.failed - s.skipped)

        finished =
            s.succeeded + s.failed + s.skipped
    in
    { session = s
    , isRunning = isRunning
    , stale = stale
    , heartbeat = heartbeat
    , duration = Format.elapsedMs now s.startedAt end |> Maybe.map Format.durationMs |> Maybe.withDefault Format.dash
    , badge =
        if stale then
            Format.badge "running · stale?" "serious"

        else
            Format.statusBadge s.status
    , inFlight = inFlight
    , finished = finished
    , progressText = Format.thousands finished ++ " of " ++ Format.thousands s.attempted ++ " finished"
    , successRate = Format.pct (Format.ratioPct s.succeeded (s.succeeded + s.failed))
    }


orElse : Maybe a -> Maybe a -> Maybe a
orElse fallback first =
    case first of
        Just _ ->
            first

        Nothing ->
            fallback


progressBar : Item -> Html msg
progressBar i =
    let
        s =
            i.session
    in
    View.segbar
        (String.fromInt s.succeeded
            ++ " succeeded, "
            ++ String.fromInt s.skipped
            ++ " skipped, "
            ++ String.fromInt s.failed
            ++ " failed, "
            ++ String.fromInt i.inFlight
            ++ " in flight"
        )
        [ { class = "seg-good", value = s.succeeded, name = "succeeded" }
        , { class = "seg-neutral", value = s.skipped, name = "skipped" }
        , { class = "seg-critical", value = s.failed, name = "failed" }
        , { class = "seg-info", value = i.inFlight, name = "in flight" }
        ]
