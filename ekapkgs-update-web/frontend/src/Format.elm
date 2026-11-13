module Format exposing
    ( Badge
    , absolute
    , addDays
    , badge
    , bytes
    , compact
    , cssPct
    , dash
    , dayKey
    , durationMs
    , durationOpt
    , elapsedMs
    , fixed1
    , href
    , humanize
    , monthDay
    , packageBadge
    , packageHref
    , parseTs
    , pct
    , phaseRank
    , phaseShort
    , phases
    , prState
    , prettyJson
    , ratioPct
    , relative
    , repoLabel
    , repoParam
    , safeUrl
    , sessionHref
    , shortId
    , shortSha
    , shortVersion
    , statusBadge
    , thousands
    , tone
    , truncate
    , upstreamUrl
    )

{-| Display helpers shared by every page: timestamps, durations, numbers,
versions, links and status tones.
-}

import Json.Decode as D
import Json.Encode as E
import Time
import Url



-- PLACEHOLDERS AND LABELS


{-| Placeholder for missing values.
-}
dash : String
dash =
    "—"


{-| Display label for a repository name: the unnamed repository
(`repo_name = ''`, used by `run` and legacy watch) is "default".
-}
repoLabel : String -> String
repoLabel name =
    if name == "" then
        "default"

    else
        name


{-| `?repo=` value for a repository (the label; the API maps "default"
back to `''`).
-}
repoParam : String -> String
repoParam =
    repoLabel


{-| `up_to_date` → `up to date`.
-}
humanize : String -> String
humanize =
    String.replace "_" " "


truncate : Int -> String -> String
truncate max s =
    if String.length s <= max then
        s

    else
        String.left (max - 1) s ++ "…"


{-| Commit-hash versions are cut to 10 characters, very long versions are
truncated, everything else is unchanged.
-}
shortVersion : String -> String
shortVersion v =
    if String.length v >= 16 && String.all Char.isHexDigit v then
        String.left 10 v

    else
        truncate 24 v


{-| First 8 characters of a session id (UUID).
-}
shortId : String -> String
shortId =
    String.left 8


shortSha : String -> String
shortSha =
    String.left 10


{-| Pretty-print a JSON string; anything else is returned unchanged.
-}
prettyJson : String -> String
prettyJson raw =
    case D.decodeString D.value raw of
        Ok value ->
            E.encode 2 value

        Err _ ->
            raw



-- NUMBERS


{-| `1284` → `1,284`.
-}
thousands : Int -> String
thousands n =
    let
        group digits =
            if String.length digits <= 3 then
                digits

            else
                group (String.dropRight 3 digits) ++ "," ++ String.right 3 digits
    in
    (if n < 0 then
        "-"

     else
        ""
    )
        ++ group (String.fromInt (abs n))


{-| One decimal place: `4.25` → `4.3`.
-}
fixed1 : Float -> String
fixed1 x =
    let
        tenths =
            round (abs x * 10)

        sign =
            if x < 0 && tenths /= 0 then
                "-"

            else
                ""
    in
    sign ++ String.fromInt (tenths // 10) ++ "." ++ String.fromInt (modBy 10 tenths)


fixed2 : Float -> String
fixed2 x =
    let
        hundredths =
            round (abs x * 100)

        frac =
            String.padLeft 2 '0' (String.fromInt (modBy 100 hundredths))
    in
    String.fromInt (hundredths // 100) ++ "." ++ frac


{-| Full under 10,000, then `12.9K`, `4.2M`.
-}
compact : Int -> String
compact n =
    if abs n < 10000 then
        thousands n

    else if round (toFloat (abs n) / 100) < 10000 then
        -- Below 999,950, which would round up to "1000.0K".
        fixed1 (toFloat n / 1000) ++ "K"

    else
        fixed1 (toFloat n / 1000000) ++ "M"


{-| `num / den` as a percentage, `Nothing` when `den` is zero.
-}
ratioPct : Int -> Int -> Maybe Float
ratioPct num den =
    if den > 0 then
        Just (toFloat num * 100 / toFloat den)

    else
        Nothing


{-| `Just 42.0` → `42.0%`, `Nothing` → `—`.
-}
pct : Maybe Float -> String
pct p =
    case p of
        Just v ->
            fixed1 v ++ "%"

        Nothing ->
            dash


{-| A CSS percentage for inline widths/heights, clamped to 0–100.
-}
cssPct : Float -> Float -> String
cssPct num den =
    if den <= 0 then
        "0%"

    else
        fixed2 (clamp 0 100 (num / den * 100)) ++ "%"


{-| `812 B`, `4.2 KB`, `1.3 MB`.
-}
bytes : Int -> String
bytes n =
    if n < 1024 then
        String.fromInt n ++ " B"

    else if round (toFloat n * 10 / 1024) < 10240 then
        -- Below 1023.95 KB, which would round up to "1024.0 KB".
        fixed1 (toFloat n / 1024) ++ " KB"

    else
        fixed1 (toFloat n / (1024 * 1024)) ++ " MB"



-- DURATIONS


{-| `850ms`, `4.2s`, `1m 23s`, `2h 5m`, `3d 4h`.
-}
durationMs : Int -> String
durationMs ms =
    let
        s =
            ms // 1000
    in
    if ms < 0 then
        dash

    else if ms < 1000 then
        String.fromInt ms ++ "ms"

    else if ms < 10000 then
        fixed1 (toFloat ms / 1000) ++ "s"

    else if s < 60 then
        String.fromInt s ++ "s"

    else if s < 3600 then
        String.fromInt (s // 60) ++ "m " ++ String.fromInt (modBy 60 s) ++ "s"

    else if s < 86400 then
        String.fromInt (s // 3600) ++ "h " ++ String.fromInt (modBy 3600 s // 60) ++ "m"

    else
        String.fromInt (s // 86400) ++ "d " ++ String.fromInt (modBy 86400 s // 3600) ++ "h"


durationOpt : Maybe Int -> String
durationOpt =
    Maybe.map durationMs >> Maybe.withDefault dash


{-| Milliseconds from `start` to `end` (or to `now` when `end` is
`Nothing`), never negative.
-}
elapsedMs : Time.Posix -> String -> Maybe String -> Maybe Int
elapsedMs now start end =
    let
        endTime =
            case end of
                Just e ->
                    parseTs e

                Nothing ->
                    Just now
    in
    Maybe.map2 (\a b -> max 0 (Time.posixToMillis b - Time.posixToMillis a)) (parseTs start) endTime



-- TIMESTAMPS


{-| Parse a stored timestamp: RFC 3339 (`2026-11-11T08:40:12.123+00:00`,
`…Z`) as written by chrono, or SQLite's `YYYY-MM-DD HH:MM:SS[.fff]`
(implicitly UTC).
-}
parseTs : String -> Maybe Time.Posix
parseTs raw =
    let
        s =
            String.trim raw

        num from len =
            String.toInt (String.slice from (from + len) s)

        sep =
            String.slice 10 11 s

        rest =
            String.dropLeft 19 s

        ( fracDigits, zone ) =
            if String.startsWith "." rest then
                let
                    digits =
                        String.dropLeft 1 rest |> String.toList |> takeWhile Char.isDigit |> String.fromList
                in
                ( digits, String.dropLeft (1 + String.length digits) rest )

            else
                ( "", rest )

        fracMs =
            String.toInt (String.left 3 (fracDigits ++ "000")) |> Maybe.withDefault 0

        offsetMinutes =
            case zone of
                "" ->
                    Just 0

                "Z" ->
                    Just 0

                "z" ->
                    Just 0

                _ ->
                    let
                        sign =
                            String.left 1 zone

                        hh =
                            String.toInt (String.slice 1 3 zone)

                        mm =
                            String.toInt (String.slice 4 6 zone)

                        valid =
                            String.length zone == 6 && String.slice 3 4 zone == ":"
                    in
                    case ( sign, Maybe.map2 Tuple.pair hh mm, valid ) of
                        ( "+", Just ( h, m ), True ) ->
                            Just (h * 60 + m)

                        ( "-", Just ( h, m ), True ) ->
                            Just -(h * 60 + m)

                        _ ->
                            Nothing
    in
    if (sep == "T" || sep == " ") && String.slice 4 5 s == "-" && String.slice 13 14 s == ":" then
        Maybe.map5
            (\y mo d ( h, mi, sec ) off ->
                let
                    seconds =
                        daysFromCivil y mo d * 86400 + h * 3600 + mi * 60 + sec - off * 60
                in
                Time.millisToPosix (seconds * 1000 + fracMs)
            )
            (num 0 4)
            (num 5 2)
            (num 8 2)
            (Maybe.map3 (\a b c -> ( a, b, c )) (num 11 2) (num 14 2) (num 17 2))
            offsetMinutes

    else
        Nothing


takeWhile : (a -> Bool) -> List a -> List a
takeWhile ok list =
    case list of
        x :: xs ->
            if ok x then
                x :: takeWhile ok xs

            else
                []

        [] ->
            []


{-| Days since 1970-01-01 for a proleptic Gregorian date.
-}
daysFromCivil : Int -> Int -> Int -> Int
daysFromCivil y m d =
    let
        y2 =
            if m <= 2 then
                y - 1

            else
                y

        era =
            (if y2 >= 0 then
                y2

             else
                y2 - 399
            )
                // 400

        yoe =
            y2 - era * 400

        mp =
            modBy 12 (m + 9)

        doy =
            (153 * mp + 2) // 5 + d - 1

        doe =
            yoe * 365 + yoe // 4 - yoe // 100 + doy
    in
    era * 146097 + doe - 719468


{-| "just now", "5m ago", "3h ago", "in 2d".
-}
relative : Time.Posix -> Time.Posix -> String
relative now ts =
    let
        secs =
            (Time.posixToMillis now - Time.posixToMillis ts) // 1000

        future =
            secs < 0

        a =
            abs secs

        body =
            if a < 3570 then
                -- Rounded to the nearest minute, so stop before "60m".
                String.fromInt ((a + 30) // 60) ++ "m"

            else if a < 86400 then
                String.fromInt (max 1 (a // 3600)) ++ "h"

            else if a < 86400 * 30 then
                String.fromInt (a // 86400) ++ "d"

            else if a < 86400 * 365 then
                String.fromInt (a // (86400 * 30)) ++ "mo"

            else
                String.fromInt (a // (86400 * 365)) ++ "y"
    in
    if a < 45 then
        if future then
            "in a moment"

        else
            "just now"

    else if future then
        "in " ++ body

    else
        body ++ " ago"


pad2 : Int -> String
pad2 =
    String.fromInt >> String.padLeft 2 '0'


monthNumber : Time.Month -> Int
monthNumber month =
    case month of
        Time.Jan ->
            1

        Time.Feb ->
            2

        Time.Mar ->
            3

        Time.Apr ->
            4

        Time.May ->
            5

        Time.Jun ->
            6

        Time.Jul ->
            7

        Time.Aug ->
            8

        Time.Sep ->
            9

        Time.Oct ->
            10

        Time.Nov ->
            11

        Time.Dec ->
            12


monthName : Time.Month -> String
monthName month =
    case month of
        Time.Jan ->
            "Jan"

        Time.Feb ->
            "Feb"

        Time.Mar ->
            "Mar"

        Time.Apr ->
            "Apr"

        Time.May ->
            "May"

        Time.Jun ->
            "Jun"

        Time.Jul ->
            "Jul"

        Time.Aug ->
            "Aug"

        Time.Sep ->
            "Sep"

        Time.Oct ->
            "Oct"

        Time.Nov ->
            "Nov"

        Time.Dec ->
            "Dec"


{-| `2026-11-11 08:40:12 UTC`.
-}
absolute : Time.Posix -> String
absolute t =
    dayKey t
        ++ " "
        ++ pad2 (Time.toHour Time.utc t)
        ++ ":"
        ++ pad2 (Time.toMinute Time.utc t)
        ++ ":"
        ++ pad2 (Time.toSecond Time.utc t)
        ++ " UTC"


{-| The UTC date as `YYYY-MM-DD` (SQLite's `date()`).
-}
dayKey : Time.Posix -> String
dayKey t =
    String.fromInt (Time.toYear Time.utc t)
        ++ "-"
        ++ pad2 (monthNumber (Time.toMonth Time.utc t))
        ++ "-"
        ++ pad2 (Time.toDay Time.utc t)


{-| `Nov 3` (UTC).
-}
monthDay : Time.Posix -> String
monthDay t =
    monthName (Time.toMonth Time.utc t) ++ " " ++ String.fromInt (Time.toDay Time.utc t)


addDays : Int -> Time.Posix -> Time.Posix
addDays n t =
    Time.millisToPosix (Time.posixToMillis t + n * 86400000)



-- LINKS


{-| Join a path and query pairs, skipping empty values.
-}
href : String -> List ( String, String ) -> String
href path pairs =
    case List.filter (\( _, v ) -> v /= "") pairs of
        [] ->
            path

        kept ->
            path ++ "?" ++ String.join "&" (List.map (\( k, v ) -> k ++ "=" ++ Url.percentEncode v) kept)


packageHref : String -> String -> String
packageHref repoName attrPath =
    href ("/packages/" ++ Url.percentEncode attrPath) [ ( "repo", repoParam repoName ) ]


sessionHref : String -> String
sessionHref id =
    "/sessions/" ++ Url.percentEncode id


{-| The URL when it is a plain http(s) link (safe for `href`).
-}
safeUrl : Maybe String -> Maybe String
safeUrl url =
    url
        |> Maybe.map String.trim
        |> Maybe.andThen
            (\u ->
                if String.startsWith "https://" u || String.startsWith "http://" u then
                    Just u

                else
                    Nothing
            )


{-| Web page for an upstream source, when one can be derived.
-}
upstreamUrl : String -> String -> Maybe String -> Maybe String
upstreamUrl kind key instance =
    let
        host =
            instance
                |> Maybe.andThen
                    (\i ->
                        if i == "" then
                            Nothing

                        else
                            Just i
                    )
                |> Maybe.withDefault "gitlab.com"
                |> String.replace "https://" ""
                |> String.replace "http://" ""
    in
    case kind of
        "github" ->
            safeUrl (Just ("https://github.com/" ++ key ++ "/releases"))

        "gitlab" ->
            safeUrl (Just ("https://" ++ host ++ "/" ++ key ++ "/-/tags"))

        "sourcehut" ->
            safeUrl (Just ("https://git.sr.ht/" ++ key ++ "/refs"))

        "pypi" ->
            safeUrl (Just ("https://pypi.org/project/" ++ key ++ "/#history"))

        "directory" ->
            safeUrl (Just key)

        _ ->
            Nothing



-- STATUS TONES


{-| Visual tone for a status value (a `tone-*` CSS class). Every status the
CLI writes (package, session, phase, watch, autofix) has a mapping;
unknown values are neutral.
-}
tone : String -> String
tone status =
    case status of
        "up_to_date" ->
            "good"

        "completed" ->
            "good"

        "success" ->
            "good"

        "ok" ->
            "good"

        "fixed" ->
            "good"

        "outdated" ->
            "warn"

        "backlog" ->
            "warn"

        "diverged" ->
            "warn"

        "stale" ->
            "warn"

        "upstream_error" ->
            "serious"

        "escalated" ->
            "serious"

        "interrupted" ->
            "serious"

        "llm_error" ->
            "serious"

        "parse_error" ->
            "serious"

        "apply_error" ->
            "serious"

        "failed" ->
            "critical"

        "error" ->
            "critical"

        "build_failed" ->
            "critical"

        "proposed" ->
            "info"

        "running" ->
            "info"

        "indexing" ->
            "info"

        "processing" ->
            "info"

        "in_flight" ->
            "info"

        _ ->
            "neutral"


type alias Badge =
    { label : String
    , tone : String
    }


badge : String -> String -> Badge
badge label t =
    { label = label, tone = t }


{-| Badge for a raw status value, labelled with the humanised value.
-}
statusBadge : String -> Badge
statusBadge status =
    { label = humanize status, tone = tone status }


{-| Badge for a package `updates.status`, marking backlog packages.
-}
packageBadge : String -> Bool -> Badge
packageBadge status backlog =
    if backlog then
        badge "outdated · backlog" "warn"

    else
        statusBadge status


{-| State of a package's recorded PR, from its `updates.status`. The PR URL is
kept after the proposal is merged, so only a `proposed` package has an open PR;
`up_to_date` means the tree caught up with the proposal.
-}
prState : String -> String
prState status =
    case status of
        "proposed" ->
            "open"

        "up_to_date" ->
            "merged"

        _ ->
            "earlier"



-- PHASES


{-| Every `UpdatePhase`, in workflow order.
-}
phases : List String
phases =
    [ "metadata_extraction"
    , "source_discovery"
    , "version_selection"
    , "source_hash_update"
    , "dependency_hash_update"
    , "build"
    , "patch_recovery"
    , "testing"
    , "agent_fix"
    , "pr_creation"
    ]


{-| Workflow position (unknown phases sort last).
-}
phaseRank : String -> Int
phaseRank p =
    let
        go i list =
            case list of
                x :: xs ->
                    if x == p then
                        i

                    else
                        go (i + 1) xs

                [] ->
                    i
    in
    go 0 phases


{-| Short label: `dependency_hash_update` → `dep hash`.
-}
phaseShort : String -> String
phaseShort p =
    case p of
        "metadata_extraction" ->
            "metadata"

        "source_discovery" ->
            "discovery"

        "version_selection" ->
            "version"

        "source_hash_update" ->
            "src hash"

        "dependency_hash_update" ->
            "dep hash"

        "patch_recovery" ->
            "patches"

        "testing" ->
            "tests"

        "agent_fix" ->
            "agent fix"

        "pr_creation" ->
            "PR"

        other ->
            humanize other
