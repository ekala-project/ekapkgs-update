module Page.Dashboard exposing (Data, decoder, view)

{-| `/`: package status tiles, running sessions, recent activity and a
compact watch summary (`GET /api/dashboard`).
-}

import Api
import Ctx exposing (Ctx)
import Dict exposing (Dict)
import Format
import Html exposing (..)
import Html.Attributes exposing (..)
import Json.Decode as D
import Msg exposing (Msg)
import Time
import View



-- DATA


type alias WatchSummary =
    { repos : List Api.Repo
    , lastPoll : Maybe Api.Cycle
    , sourceTypes : List Api.SourceTypeCount
    }


type alias Data =
    { windowDays : Int
    , counts : Api.StatusCounts
    , current : Api.Outcomes
    , previous : Api.Outcomes
    , running : List Api.Session
    , runningPhases : List Api.Phase
    , watch : WatchSummary
    , activity : List Api.Activity
    , recentPrs : List Api.Package
    }


decoder : D.Decoder Data
decoder =
    D.succeed Data
        |> Api.required "window_days" D.int
        |> Api.required "counts" Api.statusCounts
        |> Api.required "current" Api.outcomes
        |> Api.required "previous" Api.outcomes
        |> Api.required "running" (D.list Api.session)
        |> Api.required "running_phases" (D.list Api.phase)
        |> Api.required "watch" watchSummary
        |> Api.required "activity" (D.list Api.activity)
        |> Api.required "recent_prs" (D.list Api.package)


watchSummary : D.Decoder WatchSummary
watchSummary =
    D.succeed WatchSummary
        |> Api.required "repos" (D.list Api.repo)
        |> Api.nullable "last_poll" Api.cycle
        |> Api.required "source_types" (D.list Api.sourceTypeCount)



-- VIEW


view : Ctx -> Data -> Ctx.Page
view ctx data =
    { title = "Dashboard"
    , body =
        [ View.pageHead "Dashboard" "Package update status across every tracked repository."
        , section [ attribute "aria-label" "Package status" ] (viewStats data)
        , div [ class "grid-2" ]
            [ div [ class "stack" ]
                [ panel "h-active" "Running sessions" ( "/sessions?status=running", "All running" ) [ viewActive ctx.now data ]
                , panel "h-activity" "Recent activity" ( "/sessions", "All sessions" ) (viewActivity ctx.now data)
                ]
            , panel "h-watch" "Watch" ( "/watch", "Details" ) (viewWatch ctx.now data.watch)
            ]
        ]
    }


panel : String -> String -> ( String, String ) -> List (Html Msg) -> Html Msg
panel hid heading ( linkHref, linkLabel ) content =
    section [ class "panel", attribute "aria-labelledby" hid ]
        (header [ class "panel-head" ]
            [ h2 [ id hid ] [ text heading ]
            , a [ class "small", href linkHref ] [ text linkLabel ]
            ]
            :: content
        )


packagesLink : String -> String
packagesLink status =
    Format.href "/packages" [ ( "status", status ) ]



-- STATS


type alias Tile =
    { label : String
    , value : Int
    , href : String
    , tone : String
    , note : String
    , noteHref : Maybe String
    }


successRate : Api.Outcomes -> Maybe Float
successRate o =
    Format.ratioPct o.succeeded (o.succeeded + o.failed)


viewStats : Data -> List (Html Msg)
viewStats data =
    let
        c =
            data.counts

        days =
            String.fromInt data.windowDays

        isEmpty =
            c.total == 0

        rate =
            successRate data.current

        tiles =
            [ { label = "Tracked"
              , value = c.total
              , href = "/packages"
              , tone = "neutral"
              , note = ifPositive c.pending (Format.thousands c.pending ++ " pending")
              , noteHref = whenPositive c.pending (packagesLink "pending")
              }
            , { label = "Up to date"
              , value = c.upToDate
              , href = packagesLink "up_to_date"
              , tone = "good"
              , note = Format.pct (Format.ratioPct c.upToDate c.total)
              , noteHref = Nothing
              }
            , { label = "Outdated"
              , value = c.outdated
              , href = packagesLink "outdated"
              , tone = "warn"
              , note = ifPositive c.backlog (Format.thousands c.backlog ++ " backlog")
              , noteHref = whenPositive c.backlog (packagesLink "backlog")
              }
            , { label = "Proposed"
              , value = c.proposed
              , href = packagesLink "proposed"
              , tone = "info"
              , note = Format.thousands c.openPrs ++ " with open PR"
              , noteHref = Nothing
              }
            , { label = "Failed"
              , value = c.failed
              , href = packagesLink "failed"
              , tone = "critical"
              , note = ""
              , noteHref = Nothing
              }
            , { label = "Upstream errors"
              , value = c.upstreamError
              , href = packagesLink "upstream_error"
              , tone = "serious"
              , note = ""
              , noteHref = Nothing
              }
            ]

        cur =
            data.current

        detail =
            if cur.sessions == 0 then
                "No finished updates in the last " ++ days ++ " days"

            else
                Format.thousands cur.succeeded
                    ++ " succeeded · "
                    ++ Format.thousands cur.failed
                    ++ " failed · "
                    ++ Format.thousands cur.skipped
                    ++ " skipped across "
                    ++ Format.thousands cur.sessions
                    ++ " sessions"

        delta =
            case ( rate, successRate data.previous ) of
                ( Just r, Just p ) ->
                    let
                        d =
                            r - p

                        deltaTone =
                            if d >= 0.05 then
                                "good"

                            else if d <= -0.05 then
                                "critical"

                            else
                                "neutral"

                        sign =
                            if d >= 0 then
                                "+"

                            else
                                ""
                    in
                    [ span [ class ("tile-delta tone-" ++ deltaTone) ]
                        [ text (sign ++ Format.fixed1 d ++ " pts vs previous " ++ days ++ " days") ]
                    ]

                _ ->
                    []

        rateTile =
            div [ class "tile tile-rate" ]
                ([ span [ class "tile-label" ] [ text ("Success rate · " ++ days ++ " days") ]
                 , span [ class "tile-value" ] [ text (Format.pct rate) ]
                 , span [ class "tile-note" ] [ text detail ]
                 ]
                    ++ delta
                )
    in
    List.concat
        [ if isEmpty then
            [ View.emptyState "No packages tracked yet"
                [ p []
                    [ text "Run "
                    , code [] [ text "ekapkgs-update run" ]
                    , text " or start "
                    , code [] [ text "ekapkgs-update watch" ]
                    , text "; packages appear here as soon as they are checked."
                    ]
                ]
            ]

          else
            []
        , [ div [ class "tiles" ] (List.map viewTile tiles ++ [ rateTile ]) ]
        , if c.backlog > 0 then
            [ p [ class "small tile-footnote" ]
                [ text "Backlog packages are outdated only because "
                , code [] [ text "watch" ]
                , text " saw the release during its observe-only bootstrap; "
                , a [ href (packagesLink "backlog") ] [ text "review the backlog" ]
                , text " or let "
                , code [] [ text "run" ]
                , text " pick them up."
                ]
            ]

          else
            []
        , if isEmpty then
            []

          else
            [ viewComposition c ]
        ]


ifPositive : Int -> String -> String
ifPositive n s =
    if n > 0 then
        s

    else
        ""


whenPositive : Int -> a -> Maybe a
whenPositive n x =
    if n > 0 then
        Just x

    else
        Nothing


viewTile : Tile -> Html Msg
viewTile t =
    div [ class "tile" ]
        (a [ class "tile-link", href t.href ]
            [ span [ class "tile-label" ]
                (if t.tone /= "neutral" then
                    [ span [ class ("dot tone-" ++ t.tone), attribute "aria-hidden" "true" ] [], text t.label ]

                 else
                    [ text t.label ]
                )
            , span [ class "tile-value" ] [ text (Format.thousands t.value) ]
            ]
            :: (if t.note == "" then
                    []

                else
                    case t.noteHref of
                        Just nh ->
                            [ a [ class "tile-note", href nh ] [ text t.note ] ]

                        Nothing ->
                            [ span [ class "tile-note" ] [ text t.note ] ]
               )
        )


viewComposition : Api.StatusCounts -> Html Msg
viewComposition c =
    let
        parts =
            [ { class = "seg-good", value = c.upToDate, name = "up to date" }
            , { class = "seg-info", value = c.proposed, name = "proposed" }
            , { class = "seg-warn", value = c.outdated - c.backlog, name = "outdated" }
            , { class = "seg-warn-light", value = c.backlog, name = "backlog" }
            , { class = "seg-critical", value = c.failed, name = "failed" }
            , { class = "seg-serious", value = c.upstreamError, name = "upstream error" }
            , { class = "seg-neutral", value = c.pending, name = "pending" }
            ]

        label =
            parts
                |> List.filter (\s -> s.value > 0)
                |> List.map (\s -> Format.thousands s.value ++ " " ++ s.name)
                |> String.join ", "

        legendItem cls name =
            li [] [ span [ class ("swatch " ++ cls) ] [], text name ]
    in
    div [ class "composition" ]
        [ View.segbar label parts
        , ul [ class "legend", attribute "aria-hidden" "true" ]
            [ legendItem "seg-good" "up to date"
            , legendItem "seg-info" "proposed"
            , legendItem "seg-warn" "outdated"
            , legendItem "seg-warn-light" "backlog"
            , legendItem "seg-critical" "failed"
            , legendItem "seg-serious" "upstream error"
            , legendItem "seg-neutral" "pending"
            ]
        , p [ class "sr-only" ] [ text label ]
        ]



-- RUNNING SESSIONS


{-| Running sessions without a heartbeat for this long are flagged.
-}
staleAfterMs : Int
staleAfterMs =
    6 * 60 * 60 * 1000


{-| Running phases listed per session card.
-}
workPerSession : Int
workPerSession =
    6


viewActive : Time.Posix -> Data -> Html Msg
viewActive now data =
    if List.isEmpty data.running then
        p [ class "muted" ]
            [ text "No session is running. Sessions appear here while "
            , code [] [ text "ekapkgs-update run" ]
            , text " or a "
            , code [] [ text "watch" ]
            , text " poll dispatches updates."
            ]

    else
        ul [ class "session-cards" ] (List.map (viewSessionCard now data.runningPhases) data.running)


viewSessionCard : Time.Posix -> List Api.Phase -> Api.Session -> Html Msg
viewSessionCard now allPhases s =
    let
        isRunning =
            s.status == "running"

        heartbeat =
            Maybe.withDefault s.startedAt s.updatedAt

        stale =
            isRunning
                && (case Format.parseTs heartbeat of
                        Just hb ->
                            Time.posixToMillis now - Time.posixToMillis hb > staleAfterMs

                        Nothing ->
                            False
                   )

        end =
            if isRunning then
                Nothing

            else
                Just (s.completedAt |> orElse s.updatedAt |> Maybe.withDefault s.startedAt)

        duration =
            Format.elapsedMs now s.startedAt end
                |> Format.durationOpt

        badge =
            if stale then
                Format.badge "running · stale?" "serious"

            else
                Format.statusBadge s.status

        finished =
            s.succeeded + s.failed + s.skipped

        inFlight =
            Basics.max 0 (s.attempted - finished)

        progressLabel =
            String.fromInt s.succeeded
                ++ " succeeded, "
                ++ String.fromInt s.skipped
                ++ " skipped, "
                ++ String.fromInt s.failed
                ++ " failed, "
                ++ String.fromInt inFlight
                ++ " in flight"

        countDot tone n name =
            span [] [ span [ class ("dot tone-" ++ tone), attribute "aria-hidden" "true" ] [], text (Format.thousands n ++ " " ++ name) ]

        mine =
            List.filter (\ph -> ph.sessionId == s.id) allPhases

        more =
            List.length mine - workPerSession
    in
    li [ class "session-card" ]
        (List.concat
            [ [ div [ class "session-card-head" ]
                    [ a [ href (Format.sessionHref s.id), class "mono" ] [ text (Format.shortId s.id) ]
                    , text " "
                    , span [ class "tag" ] [ text s.mode ]
                    , text " "
                    , View.badge badge
                    , text " "
                    , span [ class "muted small" ] [ text "started ", View.ts now (Just s.startedAt), text (" · " ++ duration) ]
                    ]
              ]
            , if s.attempted > 0 then
                [ View.segbar progressLabel
                    [ { class = "seg-good", value = s.succeeded, name = "succeeded" }
                    , { class = "seg-neutral", value = s.skipped, name = "skipped" }
                    , { class = "seg-critical", value = s.failed, name = "failed" }
                    , { class = "seg-info", value = inFlight, name = "in flight" }
                    ]
                , p [ class "small session-counts" ]
                    [ span [] [ text (Format.thousands finished ++ " of " ++ Format.thousands s.attempted ++ " finished") ]
                    , countDot "good" s.succeeded "succeeded"
                    , countDot "critical" s.failed "failed"
                    , countDot "neutral" s.skipped "skipped"
                    , countDot "info" inFlight "in flight"
                    ]
                ]

              else
                [ p [ class "small muted" ]
                    [ text
                        ("Evaluated "
                            ++ Format.thousands s.evaluated
                            ++ " packages, "
                            ++ Format.thousands s.backoff
                            ++ " in backoff; no updates attempted yet."
                        )
                    ]
                ]
            , if stale then
                [ p [ class "small notice tone-serious" ]
                    [ text "No heartbeat since "
                    , View.ts now (Just heartbeat)
                    , text ". The process may have died without closing the session."
                    ]
                ]

              else
                []
            , if List.isEmpty mine then
                []

              else
                [ ul [ class "work-list small" ]
                    (List.map (viewWork now) (List.take workPerSession mine)
                        ++ (if more > 0 then
                                [ li [ class "muted" ] [ text ("and " ++ String.fromInt more ++ " more") ] ]

                            else
                                []
                           )
                    )
                ]
            ]
        )


viewWork : Time.Posix -> Api.Phase -> Html Msg
viewWork now ph =
    li []
        (View.packageLink ph.repoName ph.attrPath
            ++ [ text " "
               , span [ class "tag" ] [ text (Format.phaseShort ph.phase) ]
               , text " "
               , span [ class "muted" ]
                    [ text (Format.elapsedMs now ph.startedAt Nothing |> Maybe.map Format.durationMs |> Maybe.withDefault "") ]
               ]
        )


{-| Package link with its repository (omitted for the unnamed one).
-}
orElse : Maybe a -> Maybe a -> Maybe a
orElse fallback m =
    case m of
        Just _ ->
            m

        Nothing ->
            fallback



-- ACTIVITY


{-| Packages listed in the activity feed.
-}
activityItems : Int
activityItems =
    20


type alias Group =
    { sessionId : String
    , repoName : String
    , attrPath : String

    -- Newest first, as the API returns them.
    , rows : List Api.Activity
    }


{-| Group rows by (session, repo, package), in first-seen order.
-}
groupActivity : List Api.Activity -> List Group
groupActivity rows =
    let
        keyOf r =
            r.phase.sessionId ++ "\u{0000}" ++ r.phase.repoName ++ "\u{0000}" ++ r.phase.attrPath

        step r ( order, groups ) =
            let
                k =
                    keyOf r
            in
            case Dict.get k groups of
                Just g ->
                    ( order, Dict.insert k { g | rows = r :: g.rows } groups )

                Nothing ->
                    ( k :: order
                    , Dict.insert k { sessionId = r.phase.sessionId, repoName = r.phase.repoName, attrPath = r.phase.attrPath, rows = [ r ] } groups
                    )

        ( keys, grouped ) =
            List.foldl step ( [], Dict.empty ) rows
    in
    keys
        |> List.reverse
        |> List.filterMap (\k -> Dict.get k grouped)
        -- Rows were prepended while folding; restore newest-first.
        |> List.map (\g -> { g | rows = List.reverse g.rows })


{-| The overall outcome of one package's phases (in execution order).
-}
outcome : List Api.Phase -> Bool -> ( String, Format.Badge )
outcome phases sessionRunning =
    case List.filter (\ph -> ph.status == "failed") phases of
        failed :: _ ->
            ( "failed", Format.badge ("failed · " ++ Format.phaseShort failed.phase) "critical" )

        [] ->
            if List.any (\ph -> ph.status == "running") phases then
                if sessionRunning then
                    ( "running", Format.badge "running" "info" )

                else
                    ( "interrupted", Format.badge "interrupted" "serious" )

            else if List.any (\ph -> ph.phase == "pr_creation" && ph.status == "success") phases then
                ( "proposed", Format.badge "proposed" "info" )

            else if not (List.isEmpty phases) && List.all (\ph -> ph.status == "skipped") phases then
                ( "skipped", Format.badge "skipped" "neutral" )

            else if
                List.any (\ph -> ph.status == "skipped") phases
                    && not (List.any (\ph -> Format.phaseRank ph.phase >= Format.phaseRank "build") phases)
            then
                -- Stopped early (e.g. no compatible release) without building.
                ( "skipped", Format.badge "skipped" "neutral" )

            else
                ( "succeeded", Format.badge "succeeded" "good" )


viewActivity : Time.Posix -> Data -> List (Html Msg)
viewActivity now data =
    let
        groups =
            groupActivity data.activity |> List.take activityItems
    in
    (if List.isEmpty groups then
        p [ class "muted" ] [ text "No update activity recorded yet." ]

     else
        ul [ class "feed" ] (List.filterMap (viewActivityItem now) groups)
    )
        :: (if List.isEmpty data.recentPrs then
                []

            else
                [ h3 [ class "subhead" ] [ text "Latest PRs" ]
                , ul [ class "feed" ] (List.map (viewPr now) data.recentPrs)
                ]
           )


viewActivityItem : Time.Posix -> Group -> Maybe (Html Msg)
viewActivityItem now g =
    case g.rows of
        [] ->
            Nothing

        latest :: _ ->
            let
                phases =
                    List.reverse (List.map .phase g.rows)

                ( kind, badge ) =
                    outcome phases (latest.sessionStatus == Just "running")

                current =
                    latest.currentVersion |> Maybe.andThen nonEmpty

                detail =
                    case kind of
                        "failed" ->
                            let
                                error =
                                    phases
                                        |> List.filter (\ph -> ph.status == "failed")
                                        |> List.head
                                        |> Maybe.andThen .errorType
                                        |> Maybe.withDefault "error"
                            in
                            case latest.failedVersion of
                                Just v ->
                                    error ++ " · target " ++ Format.shortVersion v

                                Nothing ->
                                    error

                        "proposed" ->
                            case ( latest.proposedVersion, current ) of
                                ( Just v, Just cur ) ->
                                    Format.shortVersion cur ++ " → " ++ Format.shortVersion v

                                ( Just v, Nothing ) ->
                                    "→ " ++ Format.shortVersion v

                                ( Nothing, _ ) ->
                                    ""

                        _ ->
                            ""

                prUrl =
                    if kind == "proposed" then
                        Format.safeUrl latest.prUrl

                    else
                        Nothing

                phaseLabel =
                    Format.phaseShort latest.phase.phase
            in
            Just
                (li [ class "feed-item" ]
                    (List.concat
                        [ [ View.badge badge, text " " ]
                        , View.packageLink g.repoName g.attrPath
                        , if phaseLabel == "" then
                            []

                          else
                            [ text " ", span [ class "tag", title "Last phase reached" ] [ text phaseLabel ] ]
                        , if detail == "" then
                            []

                          else
                            [ text " ", span [ class "feed-detail" ] [ text detail ] ]
                        , prLink prUrl latest.prNumber
                        , [ text " "
                          , span [ class "feed-meta muted small" ]
                                [ View.ts now (Just (Maybe.withDefault latest.phase.startedAt latest.phase.completedAt))
                                , text " · "
                                , a [ href (Format.sessionHref g.sessionId) ]
                                    [ text (Maybe.withDefault Format.dash latest.sessionMode ++ " " ++ Format.shortId g.sessionId) ]
                                ]
                          ]
                        ]
                    )
                )


viewPr : Time.Posix -> Api.Package -> Html Msg
viewPr now pkg =
    let
        state =
            Format.prState pkg.status
    in
    li [ class "feed-item" ]
        (List.concat
            [ [ View.badge (Format.packageBadge pkg.status pkg.isBacklog)
              , text " "
              , a [ href (Format.packageHref pkg.repoName pkg.attrPath), class "attr" ] [ text pkg.attrPath ]
              , text " "
              , span [ class "feed-detail" ]
                    (if state == "open" then
                        [ View.ver pkg.currentVersion, text " → ", View.ver pkg.proposedVersion ]

                     else
                        [ View.ver pkg.currentVersion ]
                    )
              ]
            , prLink (Format.safeUrl pkg.prUrl) pkg.prNumber
            , [ text " ", span [ class "tag", title "PR state" ] [ text state ] ]
            , [ text " "
              , span [ class "feed-meta muted small" ] [ View.ts now (pkg.lastChecked |> orElse pkg.lastAttempted) ]
              ]
            ]
        )


prLink : Maybe String -> Maybe Int -> List (Html Msg)
prLink url number =
    case Format.safeUrl url of
        Just u ->
            [ text " "
            , a [ href u, rel "noopener noreferrer", target "_blank" ]
                [ text (Maybe.map (\n -> "#" ++ String.fromInt n) number |> Maybe.withDefault "PR") ]
            ]

        Nothing ->
            []


nonEmpty : String -> Maybe String
nonEmpty s =
    if String.trim s == "" then
        Nothing

    else
        Just s



-- WATCH


viewWatch : Time.Posix -> WatchSummary -> List (Html Msg)
viewWatch now w =
    let
        sourcesTotal =
            List.sum (List.map .total w.sourceTypes)

        sourcesFailing =
            List.sum (List.map .failing w.sourceTypes)

        backlog =
            List.sum (List.map .backlogCount w.repos)

        configured =
            not (List.isEmpty w.repos) || w.lastPoll /= Nothing || sourcesTotal > 0

        fact label content =
            div [] [ dt [] [ text label ], dd [] content ]
    in
    if not configured then
        [ p [ class "muted" ]
            [ text "Watch mode has not run against this database. Start "
            , code [] [ text "ekapkgs-update watch" ]
            , text " to index repositories and poll upstream feeds."
            ]
        ]

    else
        dl [ class "facts" ]
            (List.concat
                [ [ fact "Last poll"
                        (case w.lastPoll of
                            Just c ->
                                [ View.statusBadge c.status
                                , text " "
                                , View.ts now (Just c.startedAt)
                                , text " "
                                , span [ class "muted small" ]
                                    [ text
                                        ("· "
                                            ++ Format.thousands c.sourcesPolled
                                            ++ " sources · "
                                            ++ Format.thousands c.newReleases
                                            ++ " new · "
                                            ++ Format.thousands c.dispatched
                                            ++ " dispatched"
                                            ++ (if c.bootstrap then
                                                    " · bootstrap"

                                                else
                                                    ""
                                               )
                                        )
                                    ]
                                ]

                            Nothing ->
                                [ span [ class "muted" ] [ text "never" ] ]
                        )
                  , fact "Upstream sources"
                        (text (Format.thousands sourcesTotal)
                            :: (if sourcesFailing > 0 then
                                    [ text " · "
                                    , a [ href "/watch#failing-sources", class "tone-text-serious" ]
                                        [ text (Format.thousands sourcesFailing ++ " failing") ]
                                    ]

                                else
                                    []
                               )
                        )
                  ]
                , if backlog > 0 then
                    [ fact "Backlog" [ a [ href (packagesLink "backlog") ] [ text (Format.thousands backlog ++ " packages") ] ] ]

                  else
                    []
                ]
            )
            :: (if List.isEmpty w.repos then
                    []

                else
                    [ table [ class "table compact" ]
                        [ thead []
                            [ tr []
                                [ th [ scope "col" ] [ text "Repo" ]
                                , th [ scope "col" ] [ text "Sync" ]
                                , th [ scope "col" ] [ text "Index" ]
                                , th [ scope "col", class "num" ] [ text "Packages" ]
                                ]
                            ]
                        , tbody [] (List.map (viewRepoRow now) w.repos)
                        ]
                    ]
               )


viewRepoRow : Time.Posix -> Api.Repo -> Html Msg
viewRepoRow now r =
    tr []
        [ td []
            (a [ href (Format.href "/packages" [ ( "repo", Format.repoParam r.name ) ]) ] [ text (Format.repoLabel r.name) ]
                :: (if r.bootstrappedAt == Nothing then
                        [ text " ", span [ class "tag" ] [ text "bootstrap pending" ] ]

                    else
                        []
                   )
            )
        , td [] [ View.statusBadge (Maybe.withDefault "unknown" r.syncStatus) ]
        , td []
            [ View.statusBadge (Maybe.withDefault "not indexed" r.indexStatus)
            , text " "
            , span [ class "muted small" ] [ View.ts now r.lastIndexedAt ]
            ]
        , td [ class "num" ] [ text (Format.thousands r.indexRows) ]
        ]
