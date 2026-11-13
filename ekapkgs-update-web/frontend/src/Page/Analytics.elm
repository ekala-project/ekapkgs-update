module Page.Analytics exposing (Data, decoder, view)

{-| `/analytics`: outcomes, phase performance and failure patterns over a
selectable range (`?range=7d|30d|90d|all`, default 30 days).
-}

import Api
import Ctx exposing (Ctx)
import Format
import Html exposing (..)
import Html.Attributes exposing (..)
import Json.Decode as D
import View


type alias Percentile =
    { phase : String
    , p50 : Maybe Int
    , p90 : Maybe Int
    }


type alias RepoStatus =
    { repoName : String
    , status : String
    , count : Int
    , backlog : Int
    }


type alias Data =
    { range : String
    , days : Int
    , trendDays : Int
    , outcomes : Api.Outcomes
    , daily : List Api.DailyOutcome
    , errorTypes : List Api.LabelCount
    , phaseStats : List Api.PhaseStat
    , percentiles : List Percentile
    , statusByRepo : List RepoStatus
    , upstreamTypes : List Api.LabelCount
    , rebuildBucketOrder : List String
    , rebuildBuckets : List Api.LabelCount
    , topRebuilds : List Api.Package
    , autofixQueue : List Api.LabelCount
    , autofixAttempts : List Api.AttemptOutcome
    , topFailing : List Api.FailingPackage
    }


decoder : D.Decoder Data
decoder =
    D.succeed Data
        |> Api.required "range" D.string
        |> Api.required "days" D.int
        |> Api.required "trend_days" D.int
        |> Api.required "outcomes" Api.outcomes
        |> Api.required "daily" (D.list Api.dailyOutcome)
        |> Api.required "error_types" (D.list Api.labelCount)
        |> Api.required "phase_stats" (D.list Api.phaseStat)
        |> Api.required "phase_percentiles"
            (D.list
                (D.succeed Percentile
                    |> Api.required "phase" D.string
                    |> Api.nullable "p50_ms" D.int
                    |> Api.nullable "p90_ms" D.int
                )
            )
        |> Api.required "status_by_repo"
            (D.list
                (D.succeed RepoStatus
                    |> Api.required "repo_name" D.string
                    |> Api.required "status" D.string
                    |> Api.required "count" D.int
                    |> Api.required "backlog" D.int
                )
            )
        |> Api.required "upstream_types" (D.list Api.labelCount)
        |> Api.required "rebuild_bucket_order" (D.list D.string)
        |> Api.required "rebuild_buckets" (D.list Api.labelCount)
        |> Api.required "top_rebuilds" (D.list Api.package)
        |> Api.required "autofix_queue" (D.list Api.labelCount)
        |> Api.required "autofix_attempts" (D.list Api.attemptOutcome)
        |> Api.required "top_failing" (D.list Api.failingPackage)


{-| Selectable ranges: (param, label).
-}
ranges : List ( String, String )
ranges =
    [ ( "7d", "7 days" ), ( "30d", "30 days" ), ( "90d", "90 days" ), ( "all", "All time" ) ]


maxTrendDays : Int
maxTrendDays =
    90


errorTypesShown : Int
errorTypesShown =
    12


view : Ctx -> Data -> Ctx.Page
view ctx d =
    let
        rangeLabel =
            ranges
                |> List.filter (\( k, _ ) -> k == d.range)
                |> List.head
                |> Maybe.map Tuple.second
                |> Maybe.withDefault "30 days"

        o =
            d.outcomes
    in
    { title = "Analytics"
    , body =
        [ View.pageHead "Analytics" "Outcomes, phase performance and failure patterns."
        , div [ class "filter-row" ]
            [ span [ class "small muted" ] [ text "Time range" ]
            , View.chips "Time range"
                (List.map
                    (\( key, label ) ->
                        View.chip label
                            Nothing
                            (Format.href "/analytics"
                                [ ( "range"
                                  , if key == "30d" then
                                        ""

                                    else
                                        key
                                  )
                                ]
                            )
                            (key == d.range)
                            "neutral"
                    )
                    ranges
                )
            ]
        , div [ class "tiles tiles-compact" ]
            [ tile [ text ("Sessions · " ++ rangeLabel) ] (Format.thousands o.sessions) "with at least one attempt"
            , tile [ text "Updates attempted" ] (Format.thousands o.attempted) ""
            , tile [ dot "good", text "Succeeded" ] (Format.thousands o.succeeded) ""
            , tile [ dot "critical", text "Failed" ] (Format.thousands o.failed) ""
            , tile [ text "Success rate" ] (Format.pct (Format.ratioPct o.succeeded (o.succeeded + o.failed))) "succeeded ÷ (succeeded + failed)"
            ]
        , trendPanel ctx d
        , phasesPanel d
        , div [ class "grid-2" ]
            [ panel "h-errors" "Error types" "failed phases" <|
                if List.isEmpty d.errorTypes then
                    [ p [ class "muted" ] [ text "No errors recorded in this period." ] ]

                else
                    [ View.hbars "Error type" (errorBars d.errorTypes) ]
            , failingPanel ctx d
            ]
        , repoPanel d
        , div [ class "grid-2" ]
            [ panel "h-upstreams" "Upstream types" "tracked packages" <|
                if List.isEmpty d.upstreamTypes then
                    [ p [ class "muted" ] [ text "No packages tracked yet." ] ]

                else
                    [ View.hbars "Upstream" (View.rankedBars (List.map (\l -> ( l.label, l.count )) d.upstreamTypes)) ]
            , rebuildPanel d
            ]
        , autofixPanel d
        ]
    }


tile : List (Html msg) -> String -> String -> Html msg
tile label value note =
    div [ class "tile" ]
        ([ span [ class "tile-label" ] label, span [ class "tile-value" ] [ text value ] ]
            ++ (if note == "" then
                    []

                else
                    [ span [ class "tile-note" ] [ text note ] ]
               )
        )


dot : String -> Html msg
dot t =
    span [ class ("dot tone-" ++ t), attribute "aria-hidden" "true" ] []


panel : String -> String -> String -> List (Html msg) -> Html msg
panel id_ heading note content =
    section [ class "panel", attribute "aria-labelledby" id_ ]
        (header [ class "panel-head" ] [ h2 [ id id_ ] [ text heading ], span [ class "small muted" ] [ text note ] ]
            :: content
        )


details_ : List (Attribute msg) -> List (Html msg) -> Html msg
details_ =
    node "details"



-- DAILY TREND


type alias Column =
    { date : String
    , label : String
    , height : String
    , succeeded : Int
    , failed : Int
    , rate : String
    , tip : String
    , hasValue : Bool
    }


{-| A "nice" tick step (1/2/5 × 10ⁿ) giving at most `target` intervals.
-}
niceStep : Int -> Int -> Int
niceStep maxValue target =
    if maxValue <= 0 || target <= 0 then
        1

    else
        let
            raw =
                toFloat maxValue / toFloat target

            mag =
                10 ^ toFloat (floor (logBase 10 raw))

            norm =
                raw / mag

            factor =
                if norm <= 1 then
                    1

                else if norm <= 2 then
                    2

                else if norm <= 5 then
                    5

                else
                    10
        in
        Basics.max 1 (round (factor * mag))


trendPanel : Ctx -> Data -> Html msg
trendPanel ctx d =
    let
        days =
            Basics.max 1 d.trendDays

        labelEvery =
            if days <= 10 then
                1

            else if days <= 45 then
                7

            else
                14

        lookup key =
            d.daily
                |> List.filter (\r -> r.day == key)
                |> List.head
                |> Maybe.map (\r -> ( r.succeeded, r.failed ))
                |> Maybe.withDefault ( 0, 0 )

        raw =
            List.range 0 (days - 1)
                |> List.map
                    (\i ->
                        let
                            date =
                                Format.addDays (i - (days - 1)) ctx.now

                            ( s, f ) =
                                lookup (Format.dayKey date)
                        in
                        ( date, s, f )
                    )

        maxTotal =
            raw |> List.map (\( _, s, f ) -> s + f) |> List.maximum |> Maybe.withDefault 0

        step =
            niceStep maxTotal 4

        axisMax =
            if maxTotal == 0 then
                step

            else
                ((maxTotal + step - 1) // step) * step

        columns =
            List.indexedMap
                (\i ( date, s, f ) ->
                    let
                        total =
                            s + f

                        rate =
                            Format.pct (Format.ratioPct s total)

                        pretty =
                            Format.monthDay date
                    in
                    { date = Format.dayKey date
                    , label =
                        -- Anchored on the newest day, so the last column is
                        -- always labelled.
                        if modBy labelEvery (days - 1 - i) == 0 then
                            pretty

                        else
                            ""
                    , height = Format.cssPct (toFloat total) (toFloat axisMax)
                    , succeeded = s
                    , failed = f
                    , rate = rate
                    , tip =
                        if total == 0 then
                            pretty ++ ": no finished updates"

                        else
                            pretty ++ ": " ++ Format.thousands s ++ " succeeded · " ++ Format.thousands f ++ " failed · " ++ rate ++ " success"
                    , hasValue = total > 0
                    }
                )
                raw

        ticks =
            List.range 0 (axisMax // step)
                |> List.map (\k -> ( Format.compact (k * step), Format.cssPct (toFloat (k * step)) (toFloat axisMax) ))

        ( sumS, sumF ) =
            List.foldl (\( _, s, f ) ( a, b ) -> ( a + s, b + f )) ( 0, 0 ) raw

        caption =
            Format.thousands sumS
                ++ " succeeded, "
                ++ Format.thousands sumF
                ++ " failed over "
                ++ String.fromInt days
                ++ " days ("
                ++ Format.pct (Format.ratioPct sumS (sumS + sumF))
                ++ " success)"

        note =
            if d.days == 0 then
                "Last " ++ String.fromInt maxTrendDays ++ " days (the trend is capped at " ++ String.fromInt maxTrendDays ++ " days)"

            else
                "Last " ++ String.fromInt d.trendDays ++ " days"
    in
    panel "h-trend" "Daily outcomes" (note ++ " · finished sessions only") <|
        if maxTotal > 0 then
            [ figure [ class "colchart" ]
                [ div [ class "colchart-plot" ]
                    (List.map
                        (\( label, bottom ) -> span [ class "gridline", style "bottom" bottom ] [ span [ class "tick" ] [ text label ] ])
                        ticks
                        ++ [ ol [ class "colchart-cols" ] (List.map viewColumn columns) ]
                    )
                , figcaption []
                    [ ul [ class "legend" ]
                        [ li [] [ span [ class "swatch col-failed" ] [], text "failed" ]
                        , li [] [ span [ class "swatch col-succeeded" ] [], text "succeeded" ]
                        ]
                    , span [ class "small muted" ] [ text caption ]
                    ]
                ]
            , details_ [ class "table-view" ]
                [ summary [] [ text "Table view" ]
                , div [ class "table-wrap" ]
                    [ table [ class "table compact" ]
                        [ thead [] [ tr [] [ th [ scope "col" ] [ text "Day" ], th [ scope "col", class "num" ] [ text "Succeeded" ], th [ scope "col", class "num" ] [ text "Failed" ], th [ scope "col", class "num" ] [ text "Success" ] ] ]
                        , tbody []
                            (columns
                                |> List.filter .hasValue
                                |> List.map
                                    (\c ->
                                        tr []
                                            [ td [] [ text c.date ]
                                            , td [ class "num" ] [ text (Format.thousands c.succeeded) ]
                                            , td [ class "num" ] [ text (Format.thousands c.failed) ]
                                            , td [ class "num" ] [ text c.rate ]
                                            ]
                                    )
                            )
                        ]
                    ]
                ]
            ]

        else
            [ p [ class "muted" ] [ text "No finished sessions with attempted updates in this period." ] ]


viewColumn : Column -> Html msg
viewColumn c =
    li [ class "col", tabindex 0, attribute "data-tip" c.tip, attribute "aria-label" c.tip ]
        ((if c.hasValue then
            [ span [ class "col-stack", style "height" c.height ]
                ((if c.failed > 0 then
                    [ span [ class "col-seg col-failed", style "flex-grow" (String.fromInt c.failed) ] [] ]

                  else
                    []
                 )
                    ++ (if c.succeeded > 0 then
                            [ span [ class "col-seg col-succeeded", style "flex-grow" (String.fromInt c.succeeded) ] [] ]

                        else
                            []
                       )
                )
            ]

          else
            []
         )
            ++ (if c.label == "" then
                    []

                else
                    [ span [ class "col-label" ] [ text c.label ] ]
               )
        )



-- PHASES


phasesPanel : Data -> Html msg
phasesPanel d =
    let
        sorted =
            List.sortBy (\p -> ( Format.phaseRank p.phase, p.phase )) d.phaseStats

        pctl phase =
            d.percentiles
                |> List.filter (\x -> x.phase == phase)
                |> List.head
                |> Maybe.map (\x -> ( x.p50, x.p90 ))
                |> Maybe.withDefault ( Nothing, Nothing )

        numTh label =
            th [ scope "col", class "num" ] [ text label ]

        row ps =
            let
                ( p50, p90 ) =
                    pctl ps.phase

                finished =
                    ps.succeeded + ps.failed

                rate =
                    Format.ratioPct ps.failed finished

                meterTone =
                    case rate of
                        Just r ->
                            if r >= 30 then
                                "critical"

                            else if r >= 10 then
                                "warn"

                            else
                                "info"

                        Nothing ->
                            "info"

                failedCell =
                    if ps.failed /= 0 && List.member ps.phase Format.phases then
                        a [ href (Format.href "/failures" [ ( "phase", ps.phase ) ]) ] [ text (Format.thousands ps.failed) ]

                    else
                        text (Format.thousands ps.failed)
            in
            tr []
                [ td [ title ps.phase ]
                    (text (Format.phaseShort ps.phase)
                        :: (if ps.running /= 0 then
                                [ text " ", span [ class "small muted" ] [ text ("(" ++ Format.thousands ps.running ++ " running)") ] ]

                            else
                                []
                           )
                    )
                , td [ class "num" ] [ text (Format.thousands ps.total) ]
                , td [ class "num" ] [ text (Format.thousands ps.succeeded) ]
                , td [ class "num" ] [ failedCell ]
                , td [ class "num" ] [ text (Format.thousands ps.skipped) ]
                , td [ class "meter-cell" ]
                    [ span [ class "meter" ]
                        [ span [ class ("meter-fill tone-bg-" ++ meterTone), style "width" (Format.cssPct (toFloat ps.failed) (toFloat finished)) ] [] ]
                    , text " "
                    , span [ class "small" ] [ text (Format.pct rate) ]
                    ]
                , td [ class "num" ] [ text (ps.avgMs |> Maybe.map (round >> Format.durationMs) |> Maybe.withDefault Format.dash) ]
                , td [ class "num" ] [ text (Format.durationOpt p50) ]
                , td [ class "num" ] [ text (Format.durationOpt p90) ]
                , td [ class "num" ] [ text (Format.durationOpt ps.maxMs) ]
                ]
    in
    panel "h-phases" "Phases" "Failure rate = failed ÷ (succeeded + failed)" <|
        if List.isEmpty sorted then
            [ p [ class "muted" ] [ text "No phases recorded in this period." ] ]

        else
            [ div [ class "table-wrap" ]
                [ table [ class "table" ]
                    [ thead []
                        [ tr []
                            [ th [ scope "col" ] [ text "Phase" ]
                            , numTh "Runs"
                            , numTh "Succeeded"
                            , numTh "Failed"
                            , numTh "Skipped"
                            , th [ scope "col" ] [ text "Failure rate" ]
                            , numTh "Average"
                            , numTh "p50"
                            , numTh "p90"
                            , numTh "Max"
                            ]
                        ]
                    , tbody [] (List.map row sorted)
                    ]
                ]
            ]



-- ERRORS AND FAILING PACKAGES


{-| The top error types, the rest folded into "other"; real types link to
their failures.
-}
errorBars : List Api.LabelCount -> List View.Bar
errorBars rows =
    let
        pairs =
            List.map (\l -> ( l.label, l.count )) rows

        folded =
            if List.length pairs <= errorTypesShown then
                pairs

            else
                List.take errorTypesShown pairs
                    ++ [ ( "other", List.sum (List.map Tuple.second (List.drop errorTypesShown pairs)) ) ]
    in
    View.rankedBars folded
        |> List.indexedMap
            (\i b ->
                if i >= errorTypesShown && b.label == "other" then
                    b

                else
                    { b | href = Just (Format.href "/failures" [ ( "error_type", b.label ) ]) }
            )


failingPanel : Ctx -> Data -> Html msg
failingPanel ctx d =
    panel "h-failing" "Most failing packages" "failure logs" <|
        if List.isEmpty d.topFailing then
            [ p [ class "muted" ] [ text "No failures recorded in this period." ] ]

        else
            [ div [ class "table-wrap" ]
                [ table [ class "table compact" ]
                    [ thead [] [ tr [] [ th [ scope "col" ] [ text "Package" ], th [ scope "col", class "num" ] [ text "Failures" ], th [ scope "col", class "num" ] [ text "Consecutive" ], th [ scope "col" ] [ text "Last" ] ] ]
                    , tbody []
                        (List.map
                            (\f ->
                                tr []
                                    [ td []
                                        (List.concat
                                            [ [ a [ href (Format.packageHref f.repoName f.attrPath), class "attr" ] [ text f.attrPath ] ]
                                            , if f.repoName /= "" then
                                                [ text " ", span [ class "muted small" ] [ text ("(" ++ f.repoName ++ ")") ] ]

                                              else
                                                []
                                            , case f.lastErrorType of
                                                Just e ->
                                                    if e /= "" then
                                                        [ text " ", span [ class "tag" ] [ text e ] ]

                                                    else
                                                        []

                                                Nothing ->
                                                    []
                                            ]
                                        )
                                    , td [ class "num" ] [ text (Format.thousands f.failures) ]
                                    , td [ class "num" ] [ text (f.consecutive |> Maybe.map Format.thousands |> Maybe.withDefault Format.dash) ]
                                    , td [] [ View.ts ctx.now (Just f.lastFailure) ]
                                    ]
                            )
                            d.topFailing
                        )
                    ]
                ]
            ]



-- STATUS BY REPOSITORY


type alias Counts =
    { total : Int
    , pending : Int
    , upToDate : Int
    , outdated : Int
    , backlog : Int
    , proposed : Int
    , failed : Int
    , upstreamError : Int
    }


emptyCounts : Counts
emptyCounts =
    { total = 0, pending = 0, upToDate = 0, outdated = 0, backlog = 0, proposed = 0, failed = 0, upstreamError = 0 }


addRow : RepoStatus -> Counts -> Counts
addRow r c0 =
    let
        c =
            { c0 | total = c0.total + r.count, backlog = c0.backlog + r.backlog }
    in
    case r.status of
        "pending" ->
            { c | pending = c.pending + r.count }

        "up_to_date" ->
            { c | upToDate = c.upToDate + r.count }

        "outdated" ->
            { c | outdated = c.outdated + r.count }

        "proposed" ->
            { c | proposed = c.proposed + r.count }

        "failed" ->
            { c | failed = c.failed + r.count }

        "upstream_error" ->
            { c | upstreamError = c.upstreamError + r.count }

        _ ->
            c


{-| Rows grouped by repository, in first-seen order.
-}
groupByRepo : List RepoStatus -> List ( String, Counts )
groupByRepo rows =
    List.foldl
        (\r acc ->
            if List.any (\( name, _ ) -> name == r.repoName) acc then
                List.map
                    (\( name, c ) ->
                        if name == r.repoName then
                            ( name, addRow r c )

                        else
                            ( name, c )
                    )
                    acc

            else
                acc ++ [ ( r.repoName, addRow r emptyCounts ) ]
        )
        []
        rows


compositionBar : Counts -> Html msg
compositionBar c =
    let
        parts =
            [ ( "seg-good", c.upToDate, "up to date" )
            , ( "seg-info", c.proposed, "proposed" )
            , ( "seg-warn", c.outdated - c.backlog, "outdated" )
            , ( "seg-warn-light", c.backlog, "backlog" )
            , ( "seg-critical", c.failed, "failed" )
            , ( "seg-serious", c.upstreamError, "upstream error" )
            , ( "seg-neutral", c.pending, "pending" )
            ]

        label =
            parts
                |> List.filter (\( _, v, _ ) -> v > 0)
                |> List.map (\( _, v, name ) -> Format.thousands v ++ " " ++ name)
                |> String.join ", "
    in
    View.segbar label (List.map (\( cls, v, name ) -> { class = cls, value = v, name = name }) parts)


repoPanel : Data -> Html msg
repoPanel d =
    let
        groups =
            groupByRepo d.statusByRepo

        swatch cls label =
            li [] [ span [ class ("swatch " ++ cls) ] [], text label ]

        numTh label =
            th [ scope "col", class "num" ] [ text label ]

        num n =
            td [ class "num" ] [ text (Format.thousands n) ]
    in
    panel "h-repos" "Status by repository" "current state, all time" <|
        if List.isEmpty groups then
            [ p [ class "muted" ] [ text "No packages tracked yet." ] ]

        else
            [ ul [ class "legend" ]
                [ swatch "seg-good" "up to date"
                , swatch "seg-info" "proposed"
                , swatch "seg-warn" "outdated"
                , swatch "seg-warn-light" "backlog"
                , swatch "seg-critical" "failed"
                , swatch "seg-serious" "upstream error"
                , swatch "seg-neutral" "pending"
                ]
            , div [ class "table-wrap" ]
                [ table [ class "table" ]
                    [ thead []
                        [ tr []
                            [ th [ scope "col" ] [ text "Repository" ]
                            , th [ scope "col", class "wide" ] [ text "Composition" ]
                            , numTh "Total"
                            , numTh "Up to date"
                            , numTh "Outdated"
                            , numTh "Backlog"
                            , numTh "Proposed"
                            , numTh "Failed"
                            , numTh "Upstream err."
                            , numTh "Pending"
                            ]
                        ]
                    , tbody []
                        (List.map
                            (\( name, c ) ->
                                let
                                    label =
                                        Format.repoLabel name
                                in
                                tr []
                                    [ td [] [ a [ href (Format.href "/packages" [ ( "repo", label ) ]) ] [ text label ] ]
                                    , td [] [ compositionBar c ]
                                    , num c.total
                                    , num c.upToDate
                                    , num c.outdated
                                    , num c.backlog
                                    , num c.proposed
                                    , num c.failed
                                    , num c.upstreamError
                                    , num c.pending
                                    ]
                            )
                            groups
                        )
                    ]
                ]
            ]



-- REBUILDS AND AUTOFIX


rebuildPanel : Data -> Html msg
rebuildPanel d =
    let
        ordered =
            List.map
                (\bucket ->
                    ( bucket
                    , d.rebuildBuckets
                        |> List.filter (\r -> r.label == bucket)
                        |> List.head
                        |> Maybe.map .count
                        |> Maybe.withDefault 0
                    )
                )
                d.rebuildBucketOrder

        hasBuckets =
            List.any (\( _, n ) -> n /= 0) ordered
    in
    panel "h-rebuilds" "Rebuild impact" "packages per rebuild count" <|
        (if hasBuckets then
            [ View.hbars "Rebuilds" (View.rankedBars ordered) ]

         else
            [ p [ class "muted" ] [ text "No packages tracked yet." ] ]
        )
            ++ (if List.isEmpty d.topRebuilds then
                    []

                else
                    [ h3 [ class "subhead" ] [ text "Largest rebuilds" ]
                    , table [ class "table compact" ]
                        [ thead [] [ tr [] [ th [ scope "col" ] [ text "Package" ], th [ scope "col" ] [ text "Status" ], th [ scope "col", class "num" ] [ text "Rebuilds" ] ] ]
                        , tbody []
                            (List.map
                                (\pkg ->
                                    tr []
                                        [ td [] [ a [ href (Format.packageHref pkg.repoName pkg.attrPath), class "attr" ] [ text pkg.attrPath ] ]
                                        , td [] [ View.badge (Format.packageBadge pkg.status pkg.isBacklog) ]
                                        , td [ class "num" ] [ text (pkg.rebuildCount |> Maybe.map Format.thousands |> Maybe.withDefault Format.dash) ]
                                        ]
                                )
                                d.topRebuilds
                            )
                        ]
                    ]
               )


autofixPanel : Data -> Html msg
autofixPanel d =
    let
        queueBars =
            View.rankedBars (List.map (\l -> ( l.label, l.count )) d.autofixQueue)
                |> List.map (\b -> { b | badge = Just (Format.statusBadge b.label) })

        numTh label =
            th [ scope "col", class "num" ] [ text label ]
    in
    panel "h-autofix" "Autofix" "LLM-assisted fix pipeline" <|
        if List.isEmpty d.autofixQueue && List.isEmpty d.autofixAttempts then
            [ p [ class "muted" ] [ text "No autofix activity in this period." ] ]

        else
            [ div [ class "grid-2" ]
                [ div []
                    [ h3 [ class "subhead" ] [ text "Queue entries by status" ]
                    , if List.isEmpty queueBars then
                        p [ class "muted" ] [ text "None." ]

                      else
                        View.hbars "Status" queueBars
                    ]
                , div []
                    [ h3 [ class "subhead" ] [ text "Attempts by outcome" ]
                    , if List.isEmpty d.autofixAttempts then
                        p [ class "muted" ] [ text "None." ]

                      else
                        table [ class "table compact" ]
                            [ thead [] [ tr [] [ th [ scope "col" ] [ text "Outcome" ], numTh "Attempts", numTh "Prompt tokens", numTh "Completion tokens" ] ]
                            , tbody []
                                (List.map
                                    (\t ->
                                        tr []
                                            [ td [] [ View.statusBadge t.status ]
                                            , td [ class "num" ] [ text (Format.thousands t.count) ]
                                            , td [ class "num" ] [ text (Format.thousands t.promptTokens) ]
                                            , td [ class "num" ] [ text (Format.thousands t.completionTokens) ]
                                            ]
                                    )
                                    d.autofixAttempts
                                )
                            ]
                    ]
                ]
            ]
