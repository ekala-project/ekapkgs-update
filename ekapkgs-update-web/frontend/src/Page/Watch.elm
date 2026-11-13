module Page.Watch exposing (Data, decoder, view)

{-| `/watch`: watched repositories, poll/index cycles and upstream source
health (`GET /api/watch?kind=&page=`).
-}

import Api
import Ctx exposing (Ctx)
import Format
import Html exposing (..)
import Html.Attributes exposing (..)
import Json.Decode as D
import Msg exposing (Msg)
import Time
import View



-- DATA


type alias CycleCounts =
    { all : Int
    , poll : Int
    , index : Int
    }


type alias Data =
    { repos : List Api.Repo
    , kind : Maybe String
    , cycles : List Api.Cycle
    , cycleTotal : Int
    , cycleCounts : CycleCounts
    , page : Int
    , perPage : Int
    , lastPoll : Maybe Api.Cycle
    , sourceTypes : List Api.SourceTypeCount
    , failingSources : List Api.Source
    , recentReleases : List Api.Source
    }


decoder : D.Decoder Data
decoder =
    D.succeed Data
        |> Api.required "repos" (D.list Api.repo)
        |> Api.nullable "kind" D.string
        |> Api.required "cycles" (D.list Api.cycle)
        |> Api.required "cycle_total" D.int
        |> Api.required "cycle_counts" cycleCounts
        |> Api.required "page" D.int
        |> Api.required "per_page" D.int
        |> Api.nullable "last_poll" Api.cycle
        |> Api.required "source_types" (D.list Api.sourceTypeCount)
        |> Api.required "failing_sources" (D.list Api.source)
        |> Api.required "recent_releases" (D.list Api.source)


cycleCounts : D.Decoder CycleCounts
cycleCounts =
    D.succeed CycleCounts
        |> Api.required "all" D.int
        |> Api.required "poll" D.int
        |> Api.required "index" D.int



-- VIEW


view : Ctx -> Data -> Ctx.Page
view ctx data =
    let
        sourcesTotal =
            List.sum (List.map .total data.sourceTypes)

        configured =
            not (List.isEmpty data.repos) || data.cycleCounts.all > 0 || sourcesTotal > 0

        awaitingBootstrap =
            List.any (\r -> r.bootstrappedAt == Nothing) data.repos
    in
    { title = "Watch"
    , body =
        View.pageHead "Watch" "Passive mode: repositories are indexed periodically and upstream feeds are polled for new releases."
            :: explainer (not configured || awaitingBootstrap)
            :: (if configured then
                    [ viewTiles ctx.now data
                    , viewRepos ctx.now data.repos
                    , viewCycles ctx data
                    , div [ class "grid-2" ]
                        [ viewTypes data.sourceTypes
                        , viewFailing ctx.now data.failingSources
                        ]
                    , viewRecent ctx.now data.recentReleases
                    ]

                else
                    [ View.emptyState "Watch mode has not run yet"
                        [ p []
                            [ text "Start "
                            , code [] [ text "ekapkgs-update watch" ]
                            , text " (or enable the "
                            , code [] [ text "ekapkgs-update-watch" ]
                            , text " service) to index repositories and poll upstream feeds."
                            ]
                        ]
                    ]
               )
    }


explainer : Bool -> Html Msg
explainer isOpen =
    node "details"
        [ class "explainer", attribute "open" "" |> when isOpen ]
        [ summary [] [ text "How watch mode works" ]
        , ol []
            [ li []
                [ strong [] [ text "Index." ]
                , text " Each repository is evaluated with "
                , code [] [ text "nix-eval-jobs" ]
                , text "; every package with a recognisable upstream source is recorded in the index."
                ]
            , li []
                [ strong [] [ text "Bootstrap (observe only)." ]
                , text " The first poll after a package is indexed records the current upstream release as a "
                , em [] [ text "baseline" ]
                , text " and dispatches nothing. Packages that were already behind upstream at that moment become "
                , a [ href "/packages?status=backlog" ] [ text "backlog" ]
                , text ": outdated, but left for "
                , code [] [ text "ekapkgs-update run" ]
                , text "."
                ]
            , li []
                [ strong [] [ text "Poll." ]
                , text " Later polls compare feeds against the baselines; only releases published after the bootstrap are dispatched as updates, each poll that dispatches creating a session."
                ]
            ]
        ]


{-| `attr` when the condition holds, a no-op attribute otherwise.
-}
when : Bool -> Attribute Msg -> Attribute Msg
when cond attr =
    if cond then
        attr

    else
        classList []


panel : String -> String -> List (Html Msg) -> Html Msg
panel hid heading content =
    section [ class "panel", attribute "aria-labelledby" hid ]
        (header [ class "panel-head" ] [ h2 [ id hid ] [ text heading ] ] :: content)


th_ : String -> Html Msg
th_ label =
    th [ scope "col" ] [ text label ]


thNum : String -> Html Msg
thNum label =
    th [ scope "col", class "num" ] [ text label ]


nonEmpty : Maybe String -> Maybe String
nonEmpty =
    Maybe.andThen
        (\s ->
            if String.trim s == "" then
                Nothing

            else
                Just s
        )


errorLine : Maybe String -> List (Html Msg)
errorLine err =
    case nonEmpty err of
        Just e ->
            [ div [ class "small tone-text-critical clip", title e ] [ text e ] ]

        Nothing ->
            []


optCount : Maybe Int -> String
optCount =
    Maybe.map Format.thousands >> Maybe.withDefault Format.dash


externalLink : String -> String -> Html Msg
externalLink url label =
    a [ href url, rel "noopener noreferrer", target "_blank" ] [ text label ]



-- TILES


viewTiles : Time.Posix -> Data -> Html Msg
viewTiles now data =
    let
        sourcesTotal =
            List.sum (List.map .total data.sourceTypes)

        failingTotal =
            List.sum (List.map .failing data.sourceTypes)

        backlog =
            List.sum (List.map .backlogCount data.repos)

        dot tone show =
            if show then
                [ span [ class ("dot tone-" ++ tone), attribute "aria-hidden" "true" ] [] ]

            else
                []

        simple label value =
            div [ class "tile" ]
                [ span [ class "tile-label" ] [ text label ]
                , span [ class "tile-value" ] [ text value ]
                ]
    in
    div [ class "tiles tiles-compact" ]
        [ simple "Repositories" (Format.thousands (List.length data.repos))
        , simple "Upstream sources" (Format.thousands sourcesTotal)
        , div [ class "tile" ]
            [ span [ class "tile-label" ] (dot "serious" (failingTotal > 0) ++ [ text "Failing sources" ])
            , span [ class "tile-value" ] [ text (Format.thousands failingTotal) ]
            ]
        , div [ class "tile" ]
            [ a [ class "tile-link", href "/packages?status=backlog" ]
                [ span [ class "tile-label" ] (dot "warn" (backlog > 0) ++ [ text "Backlog" ])
                , span [ class "tile-value" ] [ text (Format.thousands backlog) ]
                ]
            ]
        , div [ class "tile" ]
            (span [ class "tile-label" ] [ text "Last poll" ]
                :: (case data.lastPoll of
                        Just c ->
                            [ span [ class "tile-value tile-value-sm" ] [ View.ts now (Just c.startedAt) ]
                            , span [ class "tile-note" ]
                                [ View.statusBadge c.status
                                , text
                                    (" "
                                        ++ Format.thousands c.newReleases
                                        ++ " new · "
                                        ++ Format.thousands c.dispatched
                                        ++ " dispatched"
                                    )
                                ]
                            ]

                        Nothing ->
                            [ span [ class "tile-value tile-value-sm muted" ] [ text "never" ] ]
                   )
            )
        ]



-- REPOSITORIES


viewRepos : Time.Posix -> List Api.Repo -> Html Msg
viewRepos now repos =
    panel "h-repos"
        "Repositories"
        [ if List.isEmpty repos then
            p [ class "muted" ] [ text "No repositories recorded yet." ]

          else
            div [ class "table-wrap" ]
                [ table [ class "table" ]
                    [ thead []
                        [ tr []
                            [ th_ "Repository"
                            , th_ "Checkout"
                            , th_ "Sync"
                            , th_ "Index"
                            , thNum "Evaluated"
                            , thNum "Indexed"
                            , thNum "Skipped"
                            , th_ "Bootstrap"
                            , thNum "Backlog"
                            ]
                        ]
                    , tbody [] (List.map (viewRepo now) repos)
                    ]
                ]
        ]


viewRepo : Time.Posix -> Api.Repo -> Html Msg
viewRepo now r =
    let
        label =
            Format.repoParam r.name

        upstream =
            nonEmpty r.upstream

        branch =
            nonEmpty r.baseBranch

        fork =
            nonEmpty r.forkRemote

        head =
            nonEmpty r.headCommit

        indexErrors =
            Maybe.withDefault 0 r.indexErrors
    in
    tr []
        [ td []
            (a [ href (Format.href "/packages" [ ( "repo", label ) ]) ] [ strong [] [ text (Format.repoLabel r.name) ] ]
                :: (case ( Format.safeUrl upstream, upstream ) of
                        ( Just u, Just up ) ->
                            [ div [ class "small" ] [ externalLink u up ] ]

                        ( Nothing, Just up ) ->
                            [ div [ class "small muted" ] [ text up ] ]

                        _ ->
                            []
                   )
            )
        , td [ class "small" ]
            (List.concat
                [ [ code [] [ text r.path ] ]
                , if r.entrypoint /= "" && r.entrypoint /= r.path then
                    [ div [ class "muted" ] [ text "entry ", code [] [ text r.entrypoint ] ] ]

                  else
                    []
                , case branch of
                    Just b ->
                        [ div []
                            (text "branch "
                                :: code [] [ text b ]
                                :: (case fork of
                                        Just f ->
                                            [ text " · fork ", code [] [ text f ] ]

                                        Nothing ->
                                            []
                                   )
                            )
                        ]

                    Nothing ->
                        []
                , case head of
                    Just sha ->
                        [ div [ class "muted", title sha ]
                            [ code [] [ text (Format.shortSha sha) ]
                            , text (" " ++ (r.headSubject |> Maybe.map (Format.truncate 80) |> Maybe.withDefault ""))
                            ]
                        ]

                    Nothing ->
                        []
                ]
            )
        , td []
            (View.statusBadge (Maybe.withDefault "unknown" r.syncStatus)
                :: div [ class "small muted" ] [ View.ts now r.lastSyncedAt ]
                :: errorLine r.syncError
            )
        , td []
            (View.statusBadge (Maybe.withDefault "not indexed" r.indexStatus)
                :: div [ class "small muted" ] [ View.ts now r.lastIndexedAt, text (" · " ++ Format.durationOpt r.indexDurationMs) ]
                :: errorLine r.indexError
            )
        , td [ class "num" ] [ text (optCount r.evaluatedCount) ]
        , td [ class "num" ]
            (text (optCount r.indexedCount)
                :: (if indexErrors > 0 then
                        [ div [ class "small tone-text-critical" ] [ text (Format.thousands indexErrors ++ " eval errors") ] ]

                    else
                        []
                   )
            )
        , td [ class "num" ] [ text (optCount r.indexSkipped) ]
        , td []
            [ case r.bootstrappedAt of
                Just _ ->
                    View.ts now r.bootstrappedAt

                Nothing ->
                    span [ class "tag" ] [ text "pending" ]
            ]
        , td [ class "num" ]
            [ if r.backlogCount > 0 then
                a [ href (Format.href "/packages" [ ( "repo", label ), ( "status", "backlog" ) ]) ]
                    [ text (Format.thousands r.backlogCount) ]

              else
                span [ class "muted" ] [ text "0" ]
            ]
        ]



-- CYCLES


viewCycles : Ctx -> Data -> Html Msg
viewCycles ctx data =
    let
        current =
            Maybe.withDefault "" data.kind

        kindChips =
            [ View.chip "All cycles" (Just data.cycleCounts.all) "/watch" (current == "") "neutral"
            , View.chip "poll" (Just data.cycleCounts.poll) (Format.href "/watch" [ ( "kind", "poll" ) ]) (current == "poll") "neutral"
            , View.chip "index" (Just data.cycleCounts.index) (Format.href "/watch" [ ( "kind", "index" ) ]) (current == "index") "neutral"
            ]
    in
    panel "h-cycles"
        "Cycles"
        [ View.chips "Filter cycles" kindChips
        , if List.isEmpty data.cycles then
            p [ class "muted" ] [ text "No cycles recorded." ]

          else
            div [ class "table-wrap" ]
                [ table [ class "table" ]
                    [ thead []
                        [ tr []
                            [ th_ "Cycle"
                            , th_ "Status"
                            , th_ "Started"
                            , th_ "Duration"
                            , thNum "Sources"
                            , thNum "Packages"
                            , thNum "New releases"
                            , thNum "Dispatched"
                            , thNum "Updated / failed"
                            , th_ "Session"
                            ]
                        ]
                    , tbody [] (List.map (viewCycle ctx.now) data.cycles)
                    ]
                ]
        , View.pager ctx { page = data.page, perPage = data.perPage, total = data.cycleTotal }
        ]


cycleDuration : Time.Posix -> Api.Cycle -> String
cycleDuration now c =
    case c.durationMs of
        Just ms ->
            Format.durationMs ms

        Nothing ->
            if c.status == "running" then
                Format.elapsedMs now c.startedAt Nothing
                    |> Maybe.map (\ms -> Format.durationMs ms ++ " so far")
                    |> Maybe.withDefault Format.dash

            else
                Format.dash


viewCycle : Time.Posix -> Api.Cycle -> Html Msg
viewCycle now c =
    tr []
        [ td []
            (text ("#" ++ String.fromInt c.id ++ " ")
                :: span [ class "tag" ] [ text c.kind ]
                :: text " "
                :: span [ class "small muted" ] [ text (c.repoName |> Maybe.map Format.repoLabel |> Maybe.withDefault "all repos") ]
                :: (if c.bootstrap then
                        [ text " ", span [ class "tag tag-warn", title "Recorded baselines; observe only" ] [ text "bootstrap" ] ]

                    else
                        []
                   )
            )
        , td [] (View.statusBadge c.status :: errorLine c.error)
        , td [] [ View.ts now (Just c.startedAt) ]
        , td [] [ text (cycleDuration now c) ]
        , td [ class "num" ]
            (text (Format.thousands c.sourcesPolled)
                :: (if c.sourcesFailed > 0 then
                        [ text " ", span [ class "tone-text-serious small" ] [ text ("(" ++ Format.thousands c.sourcesFailed ++ " failed)") ] ]

                    else
                        []
                   )
            )
        , td [ class "num" ] [ text (Format.thousands c.packagesObserved) ]
        , td [ class "num" ] [ text (Format.thousands c.newReleases) ]
        , td [ class "num" ] [ text (Format.thousands c.dispatched) ]
        , td [ class "num" ]
            [ text (Format.thousands c.updated ++ " / ")
            , if c.failed > 0 then
                span [ class "tone-text-critical" ] [ text (Format.thousands c.failed) ]

              else
                text "0"
            ]
        , td []
            [ case c.sessionId of
                Just sid ->
                    a [ href (Format.sessionHref sid), class "mono" ] [ text (Format.shortId sid) ]

                Nothing ->
                    span [ class "muted" ] [ text Format.dash ]
            ]
        ]



-- SOURCES


{-| Meter tone for a failure rate (percent).
-}
failureTone : Maybe Float -> String
failureTone rate =
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


viewTypes : List Api.SourceTypeCount -> Html Msg
viewTypes types =
    let
        maxTotal =
            List.map .total types |> List.maximum |> Maybe.withDefault 0

        tip t =
            t.upstreamType
                ++ ": "
                ++ Format.thousands t.total
                ++ " sources, "
                ++ Format.thousands t.failing
                ++ " failing, "
                ++ Format.thousands t.neverPolled
                ++ " never polled"

        bar t =
            li [ class "hbar-row" ]
                [ span [ class "hbar-label" ] [ text t.upstreamType ]
                , span [ class "hbar-track" ]
                    [ span
                        [ class "hbar-fill"
                        , style "width" (Format.cssPct (toFloat t.total) (toFloat maxTotal))
                        , attribute "data-tip" (tip t)
                        , tabindex 0
                        ]
                        []
                    ]
                , span [ class "hbar-value" ] [ text (Format.thousands t.total) ]
                ]

        row t =
            let
                rate =
                    Format.ratioPct t.failing t.total
            in
            tr []
                [ td [] [ text t.upstreamType ]
                , td [ class "num" ] [ text (Format.thousands t.total) ]
                , td [ class "num" ] [ text (Format.thousands t.neverPolled) ]
                , td []
                    [ span [ class "meter" ]
                        [ span
                            [ class ("meter-fill tone-bg-" ++ failureTone rate)
                            , style "width" (Format.cssPct (toFloat t.failing) (toFloat t.total))
                            ]
                            []
                        ]
                    , text " "
                    , span [ classList [ ( "small", True ), ( "tone-text-serious", t.failing > 0 ) ] ]
                        [ text (Format.thousands t.failing ++ " · " ++ Format.pct rate) ]
                    ]
                ]
    in
    panel "h-types"
        "Sources by type"
        (if List.isEmpty types then
            [ p [ class "muted" ] [ text "No upstream sources polled yet." ] ]

         else
            [ ol [ class "hbars" ] (List.map bar types)
            , table [ class "table compact" ]
                [ thead [] [ tr [] [ th_ "Type", thNum "Sources", thNum "Never polled", th_ "Failing" ] ]
                , tbody [] (List.map row types)
                ]
            ]
        )


sourceBadge : Api.Source -> Format.Badge
sourceBadge s =
    if s.consecutiveFailures > 0 then
        Format.badge ("failing ×" ++ String.fromInt s.consecutiveFailures)
            (if s.consecutiveFailures >= 3 then
                "critical"

             else
                "serious"
            )

    else if s.lastPolledAt == Nothing then
        Format.badge "not polled" "neutral"

    else
        Format.badge "ok" "good"


sourceName : Api.Source -> List (Html Msg)
sourceName s =
    [ span [ class "tag" ] [ text s.upstreamType ]
    , text " "
    , case Format.upstreamUrl s.upstreamType s.upstreamKey (Just s.instance) of
        Just u ->
            externalLink u s.upstreamKey

        Nothing ->
            text s.upstreamKey
    ]


viewFailing : Time.Posix -> List Api.Source -> Html Msg
viewFailing now sources =
    section [ class "panel", attribute "aria-labelledby" "failing-sources" ]
        [ header [ class "panel-head" ] [ h2 [ id "failing-sources" ] [ text "Failing sources" ] ]
        , if List.isEmpty sources then
            p [ class "muted" ] [ text "Every polled source answered on its last poll." ]

          else
            div [ class "table-wrap" ]
                [ table [ class "table compact" ]
                    [ thead [] [ tr [] [ th_ "Source", th_ "Failures", th_ "Last success", thNum "Packages" ] ]
                    , tbody []
                        (List.map
                            (\s ->
                                tr []
                                    [ td [] (sourceName s ++ errorLine s.lastError)
                                    , td [] [ View.badge (sourceBadge s) ]
                                    , td [] [ View.ts now s.lastSuccessAt ]
                                    , td [ class "num" ] [ text (Format.thousands s.packageCount) ]
                                    ]
                            )
                            sources
                        )
                    ]
                ]
        ]


viewRecent : Time.Posix -> List Api.Source -> Html Msg
viewRecent now sources =
    panel "h-releases"
        "Recently changed releases"
        [ if List.isEmpty sources then
            p [ class "muted" ] [ text "No release changes observed yet." ]

          else
            div [ class "table-wrap" ]
                [ table [ class "table compact" ]
                    [ thead []
                        [ tr []
                            [ th_ "Source"
                            , th_ "Latest"
                            , th_ "Changed"
                            , thNum "Releases"
                            , thNum "Packages"
                            , th_ "Poll"
                            ]
                        ]
                    , tbody [] (List.map (viewRecentRow now) sources)
                    ]
                ]
        ]


viewRecentRow : Time.Posix -> Api.Source -> Html Msg
viewRecentRow now s =
    let
        tag =
            nonEmpty s.latestTag
                |> Maybe.andThen
                    (\t ->
                        if Just t == s.latestVersion then
                            Nothing

                        else
                            Just t
                    )
    in
    tr []
        [ td []
            (sourceName s
                ++ (if s.instance /= "" then
                        [ text " ", span [ class "muted small" ] [ text ("@ " ++ s.instance) ] ]

                    else
                        []
                   )
            )
        , td []
            (View.ver s.latestVersion
                :: (case tag of
                        Just t ->
                            [ text " ", span [ class "muted small" ] [ text t ] ]

                        Nothing ->
                            []
                   )
            )
        , td [] [ View.ts now s.latestChangedAt ]
        , td [ class "num" ] [ text (optCount s.releaseCount) ]
        , td [ class "num" ] [ text (Format.thousands s.packageCount) ]
        , td [] [ View.badge (sourceBadge s) ]
        ]
