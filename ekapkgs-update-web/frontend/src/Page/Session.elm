module Page.Session exposing (Data, decoder, view)

{-| One session (`/sessions/:id`): summary, per-package rollup (filtered
by `?outcome=` and paginated by `?page=` client-side), errors, the watch
poll cycle that created it, failure logs and the recorded configuration.
-}

import Api
import Ctx exposing (Ctx)
import Format
import Html exposing (..)
import Html.Attributes exposing (..)
import Json.Decode as D
import Page.Session.Item as Item
import Url
import View


type alias Data =
    { session : Api.SessionDetail
    , phases : List Api.Phase
    , logs : List Api.Log
    , cycle : Maybe Api.Cycle
    }


decoder : D.Decoder Data
decoder =
    D.map4 Data
        (D.field "session" Api.sessionDetail)
        (D.field "phases" (D.list Api.phase))
        (D.field "logs" (D.list Api.log))
        (D.field "cycle" (D.nullable Api.cycle))


rollupPerPage : Int
rollupPerPage =
    100


maxErrors : Int
maxErrors =
    50


outcomes : List String
outcomes =
    [ "succeeded", "proposed", "failed", "skipped", "running", "interrupted" ]


outcomeTone : String -> String
outcomeTone o =
    case o of
        "succeeded" ->
            "good"

        "proposed" ->
            "info"

        "running" ->
            "info"

        "failed" ->
            "critical"

        "interrupted" ->
            "serious"

        _ ->
            "neutral"



-- ROLLUP


type alias PackageRow =
    { repoName : String
    , attrPath : String
    , phases : List Api.Phase
    , outcome : String
    , badge : Format.Badge
    , totalMs : Int
    }


{-| Phases grouped by (repo, attr) in first-seen order.
-}
group : List Api.Phase -> List ( ( String, String ), List Api.Phase )
group phases =
    let
        step p ( order, groups ) =
            let
                key =
                    ( p.repoName, p.attrPath )
            in
            case lookup key groups of
                Just _ ->
                    ( order
                    , List.map
                        (\( k, ps ) ->
                            if k == key then
                                ( k, p :: ps )

                            else
                                ( k, ps )
                        )
                        groups
                    )

                Nothing ->
                    ( key :: order, ( key, [ p ] ) :: groups )

        ( keys, grouped ) =
            List.foldl step ( [], [] ) phases
    in
    List.reverse keys
        |> List.filterMap (\k -> lookup k grouped |> Maybe.map (\ps -> ( k, List.reverse ps )))


lookup : k -> List ( k, v ) -> Maybe v
lookup key pairs =
    case pairs of
        [] ->
            Nothing

        ( k, v ) :: rest ->
            if k == key then
                Just v

            else
                lookup key rest


outcomeOf : Bool -> List Api.Phase -> ( String, Format.Badge )
outcomeOf sessionRunning phases =
    let
        failed =
            List.filter (\p -> p.status == "failed") phases

        has pred =
            List.any pred phases

        buildRank =
            Format.phaseRank "build"
    in
    case failed of
        f :: _ ->
            ( "failed", Format.badge ("failed · " ++ Format.phaseShort f.phase) "critical" )

        [] ->
            if has (\p -> p.status == "running") then
                if sessionRunning then
                    ( "running", Format.badge "running" "info" )

                else
                    ( "interrupted", Format.badge "interrupted" "serious" )

            else if has (\p -> p.phase == "pr_creation" && p.status == "success") then
                ( "proposed", Format.badge "proposed" "info" )

            else if List.all (\p -> p.status == "skipped") phases then
                ( "skipped", Format.badge "skipped" "neutral" )

            else if has (\p -> p.status == "skipped") && not (has (\p -> Format.phaseRank p.phase >= buildRank)) then
                ( "skipped", Format.badge "skipped" "neutral" )

            else
                ( "succeeded", Format.badge "succeeded" "good" )


rollup : Bool -> List Api.Phase -> List PackageRow
rollup sessionRunning phases =
    group phases
        |> List.map
            (\( ( repo, attr ), ps ) ->
                let
                    ( outcome, badge ) =
                        outcomeOf sessionRunning ps
                in
                { repoName = repo
                , attrPath = attr
                , phases = ps
                , outcome = outcome
                , badge = badge
                , totalMs = ps |> List.filterMap .durationMs |> List.sum
                }
            )



-- VIEW


view : Ctx -> Data -> Ctx.Page
view ctx data =
    let
        s =
            data.session.session

        i =
            Item.item ctx.now s

        short =
            Format.shortId s.id
    in
    { title = "Session " ++ short
    , body =
        [ nav [ class "breadcrumb small", attribute "aria-label" "Breadcrumb" ]
            [ a [ href "/sessions" ] [ text "Sessions" ], text " / ", span [ class "mono" ] [ text short ] ]
        , div [ class "page-head" ]
            [ h1 [] [ text "Session ", span [ class "mono" ] [ text short ] ]
            , p [ class "muted small mono" ] [ text s.id ]
            ]
        , summaryPanel ctx i
        , packagesPanel ctx i data.phases
        , errorsPanel ctx data.phases
        , cyclePanel ctx data.cycle
        , logsPanel ctx data.logs
        , configPanel data.session.configJson
        , p [ class "small muted" ]
            (let
                raw =
                    "/api/sessions/" ++ Url.percentEncode s.id ++ "/phases"
             in
             [ text "Raw phases: ", a [ href raw ] [ text raw ] ]
            )
        ]
    }


fact : String -> List (Html msg) -> Html msg
fact label content =
    div [] [ dt [] [ text label ], dd [] content ]


summaryPanel : Ctx -> Item.Item -> Html msg
summaryPanel ctx i =
    let
        s =
            i.session

        n =
            Format.thousands
    in
    section [ class "panel", attribute "aria-labelledby" "h-summary" ]
        ([ header [ class "panel-head" ]
            [ h2 [ id "h-summary" ] [ text "Summary" ]
            , span [] [ View.badge i.badge, text " ", span [ class "tag" ] [ text s.mode ] ]
            ]
         , dl [ class "facts facts-inline" ]
            (List.concat
                [ [ fact "Started" [ View.ts ctx.now (Just s.startedAt) ]
                  , fact
                        (if i.isRunning then
                            "Running for"

                         else
                            "Duration"
                        )
                        [ text i.duration ]
                  ]
                , case s.completedAt of
                    Just c ->
                        [ fact "Completed" [ View.ts ctx.now (Just c) ] ]

                    Nothing ->
                        []
                , [ fact "Heartbeat" [ View.ts ctx.now (Just i.heartbeat) ]
                  , fact "Evaluated" [ text (n s.evaluated) ]
                  , fact "In backoff" [ text (n s.backoff) ]
                  , fact "Attempted" [ text (n s.attempted) ]
                  , fact "Succeeded" [ text (n s.succeeded) ]
                  , fact "Failed" [ text (n s.failed) ]
                  , fact "Skipped" [ text (n s.skipped) ]
                  ]
                , if i.isRunning then
                    [ fact "In flight" [ text (n i.inFlight) ] ]

                  else
                    []
                , [ fact "Success rate" [ text i.successRate ] ]
                ]
            )
         ]
            ++ (if s.attempted > 0 then
                    [ Item.progressBar i
                    , p [ class "small session-counts" ]
                        [ span [] [ text i.progressText ]
                        , countDot "good" s.succeeded "succeeded"
                        , countDot "neutral" s.skipped "skipped"
                        , countDot "critical" s.failed "failed"
                        , countDot "info" i.inFlight "in flight"
                        ]
                    ]

                else
                    []
               )
            ++ (if i.stale then
                    [ p [ class "notice tone-serious small" ]
                        [ text "This session is marked running but has had no heartbeat since "
                        , View.ts ctx.now (Just i.heartbeat)
                        , text ". The process probably exited without closing it."
                        ]
                    ]

                else
                    []
               )
        )


countDot : String -> Int -> String -> Html msg
countDot tone_ count label =
    span [ class ("tone-" ++ tone_) ]
        [ span [ class "dot", attribute "aria-hidden" "true" ] []
        , text (" " ++ Format.thousands count ++ " " ++ label)
        ]


packagesPanel : Ctx -> Item.Item -> List Api.Phase -> Html msg
packagesPanel ctx i phases =
    let
        rows =
            rollup i.isRunning phases

        requested =
            Ctx.query ctx "outcome"

        current =
            if List.member requested outcomes then
                requested

            else
                ""

        countOf o =
            List.length (List.filter (\r -> r.outcome == o) rows)

        outcomeChips =
            View.chip "All" (Just (List.length rows)) ctx.path (current == "") "neutral"
                :: List.filterMap
                    (\o ->
                        let
                            c =
                                countOf o
                        in
                        if c > 0 || o == current then
                            Just (View.chip o (Just c) (Format.href ctx.path [ ( "outcome", o ) ]) (o == current) (outcomeTone o))

                        else
                            Nothing
                    )
                    outcomes

        filtered =
            if current == "" then
                rows

            else
                List.filter (\r -> r.outcome == current) rows

        total =
            List.length filtered

        pages =
            Basics.max 1 ((total + rollupPerPage - 1) // rollupPerPage)

        page =
            clamp 1 pages (Ctx.queryInt ctx "page" 1)

        visible =
            filtered |> List.drop ((page - 1) * rollupPerPage) |> List.take rollupPerPage
    in
    section [ class "panel", attribute "aria-labelledby" "h-packages" ]
        ([ header [ class "panel-head" ]
            [ h2 [ id "h-packages" ] [ text "Packages" ]
            , span [ class "small muted" ]
                [ text (Format.thousands (List.length rows) ++ " packages · " ++ Format.thousands (List.length phases) ++ " phases") ]
            ]
         ]
            ++ (if List.length outcomeChips > 1 then
                    [ View.chips "Filter by outcome" outcomeChips ]

                else
                    []
               )
            ++ (if List.isEmpty filtered then
                    [ p [ class "muted" ]
                        [ text
                            (if current == "" then
                                "No phases recorded for this session."

                             else
                                "No packages with this outcome."
                            )
                        ]
                    ]

                else
                    [ div [ class "table-wrap" ]
                        [ table [ class "table" ]
                            [ thead []
                                [ tr []
                                    [ th [ scope "col" ] [ text "Package" ]
                                    , th [ scope "col" ] [ text "Outcome" ]
                                    , th [ scope "col" ] [ text "Phases" ]
                                    , th [ scope "col" ] [ text "Time" ]
                                    ]
                                ]
                            , tbody [] (List.map (packageRow ctx) visible)
                            ]
                        ]
                    , View.pager ctx { page = page, perPage = rollupPerPage, total = total }
                    ]
               )
        )


packageRow : Ctx -> PackageRow -> Html msg
packageRow ctx r =
    tr []
        [ td []
            (a [ class "attr", href (Format.packageHref r.repoName r.attrPath) ] [ text r.attrPath ]
                :: (if r.repoName /= "" then
                        [ text " ", span [ class "muted small" ] [ text ("(" ++ r.repoName ++ ")") ] ]

                    else
                        []
                   )
            )
        , td [] [ View.badge r.badge ]
        , td [] [ View.pchips ctx.now r.phases ]
        , td [ class "small" ]
            [ text
                (if r.totalMs > 0 then
                    Format.durationMs r.totalMs

                 else
                    ""
                )
            ]
        ]


errorsPanel : Ctx -> List Api.Phase -> Html msg
errorsPanel ctx phases =
    let
        failed =
            List.filter (\p -> p.status == "failed") phases

        more =
            List.length failed - maxErrors
    in
    if List.isEmpty failed then
        text ""

    else
        section [ class "panel", attribute "aria-labelledby" "h-errors" ]
            ([ header [ class "panel-head" ] [ h2 [ id "h-errors" ] [ text "Errors" ] ] ]
                ++ List.map (View.phaseError ctx.now True) (List.take maxErrors failed)
                ++ (if more > 0 then
                        [ p [ class "small muted" ] [ text ("and " ++ Format.thousands more ++ " more; see the raw phases.") ] ]

                    else
                        []
                   )
            )


cyclePanel : Ctx -> Maybe Api.Cycle -> Html msg
cyclePanel ctx maybeCycle =
    case maybeCycle of
        Nothing ->
            text ""

        Just c ->
            let
                duration =
                    case c.durationMs of
                        Just ms ->
                            Format.durationMs ms

                        Nothing ->
                            if c.status == "running" then
                                Format.elapsedMs ctx.now c.startedAt Nothing
                                    |> Maybe.map (\ms -> Format.durationMs ms ++ " so far")
                                    |> Maybe.withDefault Format.dash

                            else
                                Format.dash
            in
            section [ class "panel", attribute "aria-labelledby" "h-cycle" ]
                [ header [ class "panel-head" ]
                    [ h2 [ id "h-cycle" ] [ text ("Watch poll cycle #" ++ String.fromInt c.id) ]
                    , a [ class "small", href "/watch" ] [ text "Watch" ]
                    ]
                , dl [ class "facts facts-inline" ]
                    [ fact "Status" [ View.statusBadge c.status ]
                    , fact "Started" [ View.ts ctx.now (Just c.startedAt) ]
                    , fact "Duration" [ text duration ]
                    , fact "Sources polled"
                        (text (Format.thousands c.sourcesPolled)
                            :: (if c.sourcesFailed > 0 then
                                    [ text " ", span [ class "tone-text-serious" ] [ text ("(" ++ Format.thousands c.sourcesFailed ++ " failed)") ] ]

                                else
                                    []
                               )
                        )
                    , fact "New releases" [ text (Format.thousands c.newReleases) ]
                    , fact "Dispatched" [ text (Format.thousands c.dispatched) ]
                    ]
                ]


logsPanel : Ctx -> List Api.Log -> Html msg
logsPanel ctx logs =
    if List.isEmpty logs then
        text ""

    else
        section [ class "panel", attribute "aria-labelledby" "h-logs" ]
            (header [ class "panel-head" ] [ h2 [ id "h-logs" ] [ text "Failure logs" ] ]
                :: List.map (View.logEntry ctx.now True) logs
            )


configPanel : Maybe String -> Html msg
configPanel configJson =
    let
        pretty =
            configJson |> Maybe.map Format.prettyJson |> Maybe.withDefault "" |> String.trim
    in
    if pretty == "" then
        text ""

    else
        section [ class "panel" ]
            [ details []
                [ summary []
                    [ h2 [ class "inline-h" ] [ text "Configuration" ]
                    , text " "
                    , span [ class "small muted" ] [ text "as recorded when the session started" ]
                    ]
                , pre [ class "code" ] [ text pretty ]
                ]
            ]
