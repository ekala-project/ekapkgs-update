module Page.Package exposing (Data, decoder, view)

{-| `/packages/:attr?repo=`: everything known about one attribute in one
repository.
-}

import Api
import Ctx exposing (Ctx)
import Format
import Html exposing (..)
import Html.Attributes exposing (..)
import Json.Decode as D
import Time
import Url
import View


type alias Data =
    { attrPath : String
    , repoName : String
    , repos : List String
    , update : Maybe Api.Package
    , index : List Api.IndexEntry
    , sources : List Api.Source
    , phases : List Api.PackagePhase
    , logs : List Api.Log
    , queue : List Api.AutofixQueue
    , attempts : List Api.AutofixAttempt
    }


decoder : D.Decoder Data
decoder =
    D.succeed Data
        |> Api.required "attr_path" D.string
        |> Api.required "repo_name" D.string
        |> Api.required "repos" (D.list D.string)
        |> Api.nullable "update" Api.package
        |> Api.required "index" (D.list Api.indexEntry)
        |> Api.required "sources" (D.list Api.source)
        |> Api.required "phases" (D.list Api.packagePhase)
        |> Api.required "logs" (D.list Api.log)
        |> Api.andMap (D.at [ "autofix", "queue" ] (D.list Api.autofixQueue))
        |> Api.andMap (D.at [ "autofix", "attempts" ] (D.list Api.autofixAttempt))


view : Ctx -> Data -> Ctx.Page
view ctx d =
    let
        label =
            Format.repoLabel d.repoName
    in
    { title = d.attrPath
    , body =
        List.concat
            [ [ nav [ class "breadcrumb small", attribute "aria-label" "Breadcrumb" ]
                    [ a [ href "/packages" ] [ text "Packages" ], text " / ", span [] [ text d.attrPath ] ]
              , div [ class "page-head" ]
                    (h1 [ class "attr-title" ] [ text d.attrPath ]
                        :: (case d.update of
                                Just u ->
                                    View.badge (Format.packageBadge u.status u.isBacklog)
                                        :: (case u.description of
                                                Just desc ->
                                                    if desc /= "" then
                                                        [ p [ class "muted" ] [ text desc ] ]

                                                    else
                                                        []

                                                Nothing ->
                                                    []
                                           )

                                Nothing ->
                                    []
                           )
                    )
              ]
            , if List.length d.repos > 1 then
                [ div [ class "repo-switch" ]
                    [ p [ class "small muted" ]
                        [ text ("This attribute exists in " ++ String.fromInt (List.length d.repos) ++ " repositories. Showing ")
                        , strong [] [ text label ]
                        , text "."
                        ]
                    , View.chips "Repository"
                        (List.map
                            (\r -> View.chip (Format.repoLabel r) Nothing (Format.packageHref r d.attrPath) (r == d.repoName) "neutral")
                            d.repos
                        )
                    ]
                ]

              else
                []
            , [ div [ class "grid-2" ]
                    [ updatePanel ctx d
                    , upstreamPanel ctx d
                    ]
              , historyPanel ctx d
              , logsPanel ctx d label
              ]
            , if List.isEmpty d.queue then
                []

              else
                [ autofixPanel ctx d ]
            , [ p [ class "small muted" ]
                    [ text "Raw data: "
                    , let
                        api =
                            Format.href ("/api/packages/" ++ Url.percentEncode d.attrPath) [ ( "repo", label ) ]
                      in
                      a [ href api ] [ text api ]
                    ]
              ]
            ]
    }


panel : String -> List (Html msg) -> List (Html msg) -> Html msg
panel heading headExtra content =
    section [ class "panel" ]
        (header [ class "panel-head" ] (h2 [] [ text heading ] :: headExtra) :: content)


fact : String -> List (Html msg) -> Html msg
fact term content =
    div [] [ dt [] [ text term ], dd [] content ]


nonEmpty : Maybe String -> Maybe String
nonEmpty =
    Maybe.andThen
        (\s ->
            if s == "" then
                Nothing

            else
                Just s
        )



-- UPDATE STATE


updatePanel : Ctx -> Data -> Html msg
updatePanel ctx d =
    panel "Update state"
        []
        (case d.update of
            Nothing ->
                [ p [ class "muted" ] [ text "No update state recorded for this repository yet. It is known from the watch index or past sessions only." ] ]

            Just u ->
                dl [ class "facts" ] (updateFacts ctx u)
                    :: (case nonEmpty u.lastError of
                            Just err ->
                                [ div [ class "notice tone-critical" ]
                                    [ strong [] [ text "Last error" ]
                                    , pre [ class "code" ] [ text err ]
                                    ]
                                ]

                            Nothing ->
                                []
                       )
        )


updateFacts : Ctx -> Api.Package -> List (Html msg)
updateFacts ctx u =
    let
        inBackoff =
            case Maybe.andThen Format.parseTs u.nextAttempt of
                Just t ->
                    Time.posixToMillis t > Time.posixToMillis ctx.now

                Nothing ->
                    False

        when cond item =
            if cond then
                [ item ]

            else
                []

        prState =
            Format.prState u.status

        prAnchor =
            Format.safeUrl u.prUrl
                |> Maybe.map
                    (\url ->
                        a [ href url, target "_blank", rel "noopener noreferrer" ]
                            [ text
                                (case u.prNumber of
                                    Just n ->
                                        "PR #" ++ String.fromInt n

                                    Nothing ->
                                        "PR"
                                )
                            ]
                    )
    in
    List.concat
        [ [ fact "Status" [ View.badge (Format.packageBadge u.status u.isBacklog) ]
          , fact "Current version" [ View.ver u.currentVersion ]
          , fact "Latest compatible upstream" [ View.ver u.latestUpstreamVersion ]
          ]
        , when (nonEmpty u.proposedVersion /= Nothing)
            (fact "Proposed"
                (View.ver u.proposedVersion
                    :: (case ( prAnchor, prState ) of
                            ( Just link, "open" ) ->
                                [ text " ", link ]

                            _ ->
                                []
                       )
                )
            )
        , case ( prAnchor, prState ) of
            ( Just link, state ) ->
                when (state /= "open") (fact "Last PR" [ link, text " ", span [ class "muted" ] [ text state ] ])

            ( Nothing, _ ) ->
                []
        , when (nonEmpty u.baselineVersion /= Nothing)
            (fact "Watch baseline"
                (View.ver u.baselineVersion
                    :: (if u.isBacklog then
                            [ span [ class "muted small" ]
                                [ text " (backlog: seen during bootstrap, left for "
                                , code [] [ text "run" ]
                                , text ")"
                                ]
                            ]

                        else
                            []
                       )
                )
            )
        , [ fact "Last checked" [ View.ts ctx.now u.lastChecked ]
          , fact "Last attempted" [ View.ts ctx.now u.lastAttempted ]
          , fact "Next attempt"
                (View.ts ctx.now u.nextAttempt
                    :: (if inBackoff then
                            [ text " ", span [ class "tag" ] [ text "backoff" ] ]

                        else
                            []
                       )
                )
          , fact "Consecutive failures"
                (if u.failureCount > 0 then
                    span [ class "tone-text-critical" ] [ text (Format.thousands u.failureCount) ]
                        :: (if nonEmpty u.failedVersion /= Nothing then
                                [ text " targeting ", View.ver u.failedVersion ]

                            else
                                []
                           )

                 else
                    [ span [ class "muted" ] [ text "0" ] ]
                )
          , fact "Rebuilds"
                [ case u.rebuildCount of
                    Just n ->
                        text (Format.thousands n)

                    Nothing ->
                        span [ class "muted" ] [ text Format.dash ]
                ]
          ]
        , when (u.cveResolvedCount > 0 || u.cveIntroducedCount > 0)
            (fact "CVEs"
                [ text
                    (Format.thousands u.cveResolvedCount
                        ++ " resolved · "
                        ++ Format.thousands u.cveIntroducedCount
                        ++ " introduced"
                    )
                ]
            )
        , case nonEmpty u.lastSessionId of
            Just sid ->
                [ fact "Last session" [ a [ class "mono", href (Format.sessionHref sid) ] [ text (Format.shortId sid) ] ] ]

            Nothing ->
                []
        ]



-- PACKAGE AND UPSTREAM


upstreamPanel : Ctx -> Data -> Html msg
upstreamPanel ctx d =
    let
        facts =
            fact "Repository" [ text (Format.repoLabel d.repoName) ]
                :: (case d.update of
                        Just u ->
                            packageFacts u

                        Nothing ->
                            []
                   )
    in
    panel "Package and upstream"
        []
        [ dl [ class "facts" ] facts
        , h3 [ class "subhead" ] [ text "Watch index" ]
        , if List.isEmpty d.index then
            p [ class "muted small" ] [ text "Not in the watch index." ]

          else
            div [ class "table-wrap" ]
                [ table [ class "table compact" ]
                    [ thead []
                        [ tr []
                            [ th [ scope "col" ] [ text "Source" ]
                            , th [ scope "col" ] [ text "Indexed version" ]
                            , th [ scope "col" ] [ text "Selection" ]
                            , th [ scope "col" ] [ text "Indexed" ]
                            ]
                        ]
                    , tbody [] (List.map (indexRow ctx) d.index)
                    ]
                ]
        , h3 [ class "subhead" ] [ text "Source health" ]
        , if List.isEmpty d.sources then
            p [ class "muted small" ] [ text "No upstream source has been polled for this package." ]

          else
            div [ class "table-wrap" ]
                [ table [ class "table compact" ]
                    [ thead []
                        [ tr []
                            [ th [ scope "col" ] [ text "Source" ]
                            , th [ scope "col" ] [ text "Latest release" ]
                            , th [ scope "col" ] [ text "Poll" ]
                            , th [ scope "col" ] [ text "Last success" ]
                            ]
                        ]
                    , tbody [] (List.map (sourceRow ctx) d.sources)
                    ]
                ]
        ]


packageFacts : Api.Package -> List (Html msg)
packageFacts u =
    let
        upstreamLabel =
            case ( u.upstreamType, u.upstreamKey ) of
                ( Just t, Just k ) ->
                    case nonEmpty u.upstreamInstance of
                        Just i ->
                            Just (t ++ " · " ++ k ++ " @ " ++ i)

                        Nothing ->
                            Just (t ++ " · " ++ k)

                ( Just t, Nothing ) ->
                    Just t

                _ ->
                    Nothing

        upstreamLink =
            case ( u.upstreamType, u.upstreamKey ) of
                ( Just t, Just k ) ->
                    Format.upstreamUrl t k u.upstreamInstance

                _ ->
                    Nothing
    in
    List.concat
        [ case nonEmpty u.pname of
            Just pname ->
                [ fact "pname" [ code [] [ text pname ] ] ]

            Nothing ->
                []
        , case Format.safeUrl u.homepage of
            Just url ->
                [ fact "Homepage" [ a [ href url, target "_blank", rel "noopener noreferrer" ] [ text url ] ] ]

            Nothing ->
                []
        , case upstreamLabel of
            Just l ->
                [ fact "Upstream"
                    [ case upstreamLink of
                        Just url ->
                            a [ href url, target "_blank", rel "noopener noreferrer" ] [ text l ]

                        Nothing ->
                            text l
                    ]
                ]

            Nothing ->
                []
        , case nonEmpty u.semverStrategy of
            Just s ->
                [ fact "Semver strategy" [ text s ] ]

            Nothing ->
                []
        ]


sourceCell : String -> String -> Maybe String -> List (Html msg)
sourceCell kind key instance =
    List.concat
        [ [ span [ class "tag" ] [ text kind ], text " " ]
        , [ case Format.upstreamUrl kind key instance of
                Just url ->
                    a [ href url, target "_blank", rel "noopener noreferrer" ] [ text key ]

                Nothing ->
                    text key
          ]
        , case nonEmpty instance of
            Just i ->
                [ span [ class "muted small" ] [ text (" @ " ++ i) ] ]

            Nothing ->
                []
        ]


indexRow : Ctx -> Api.IndexEntry -> Html msg
indexRow ctx e =
    tr []
        [ td []
            (sourceCell e.upstreamType e.upstreamKey e.instance
                ++ (case nonEmpty e.srcUrl of
                        Just src ->
                            [ div [ class "small muted clip", title src ] [ text src ] ]

                        Nothing ->
                            []
                   )
            )
        , td [] [ View.ver e.currentVersion ]
        , td [ class "small" ]
            (List.concat
                [ [ case nonEmpty e.semverStrategy of
                        Just s ->
                            text s

                        Nothing ->
                            span [ class "muted" ] [ text "default" ]
                  ]
                , case nonEmpty e.versionRegex of
                    Just re ->
                        [ text " · regex ", code [] [ text re ] ]

                    Nothing ->
                        []
                , if e.includePrereleases then
                    [ text " · prereleases" ]

                  else
                    []
                ]
            )
        , td [] [ View.ts ctx.now (Just e.indexedAt) ]
        ]


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


sourceRow : Ctx -> Api.Source -> Html msg
sourceRow ctx s =
    tr []
        [ td [] (sourceCell s.upstreamType s.upstreamKey (Just s.instance))
        , td []
            (List.concat
                [ [ View.ver s.latestVersion ]
                , case nonEmpty s.latestTag of
                    Just tag ->
                        if Just tag /= s.latestVersion then
                            [ span [ class "muted small" ] [ text (" (" ++ tag ++ ")") ] ]

                        else
                            []

                    Nothing ->
                        []
                , if s.latestChangedAt /= Nothing then
                    [ div [ class "small muted" ] [ text "changed ", View.ts ctx.now s.latestChangedAt ] ]

                  else
                    []
                ]
            )
        , td []
            (List.concat
                [ [ View.badge (sourceBadge s), text " ", View.ts ctx.now s.lastPolledAt ]
                , case nonEmpty s.lastError of
                    Just err ->
                        [ div [ class "small tone-text-critical clip", title err ] [ text err ] ]

                    Nothing ->
                        []
                ]
            )
        , td [] [ View.ts ctx.now s.lastSuccessAt ]
        ]



-- UPDATE HISTORY


{-| Sessions shown in the history.
-}
historySessions : Int
historySessions =
    25


type alias Group =
    { sessionId : String
    , rows : List Api.PackagePhase
    }


{-| Group phase rows (newest first) by session, keeping session order;
each group's phases end up in execution order.
-}
groups : List Api.PackagePhase -> List Group
groups rows =
    List.foldl
        (\r acc ->
            if List.any (\g -> g.sessionId == r.phase.sessionId) acc then
                List.map
                    (\g ->
                        if g.sessionId == r.phase.sessionId then
                            { g | rows = r :: g.rows }

                        else
                            g
                    )
                    acc

            else
                acc ++ [ { sessionId = r.phase.sessionId, rows = [ r ] } ]
        )
        []
        rows
        |> List.take historySessions


outcome : List Api.Phase -> Bool -> Format.Badge
outcome phases running =
    case List.filter (\p -> p.status == "failed") phases of
        failed :: _ ->
            Format.badge ("failed · " ++ Format.phaseShort failed.phase) "critical"

        [] ->
            if List.any (\p -> p.status == "running") phases then
                if running then
                    Format.badge "running" "info"

                else
                    Format.badge "interrupted" "serious"

            else if List.any (\p -> p.phase == "pr_creation" && p.status == "success") phases then
                Format.badge "proposed" "info"

            else if not (List.isEmpty phases) && List.all (\p -> p.status == "skipped") phases then
                Format.badge "skipped" "neutral"

            else if
                List.any (\p -> p.status == "skipped") phases
                    && not (List.any (\p -> Format.phaseRank p.phase >= Format.phaseRank "build") phases)
            then
                -- Stopped early (e.g. no compatible release) without building.
                Format.badge "skipped" "neutral"

            else
                Format.badge "succeeded" "good"


historyPanel : Ctx -> Data -> Html msg
historyPanel ctx d =
    let
        items =
            groups d.phases
    in
    panel "Update history"
        [ span [ class "muted small" ] [ text "One row per session, phases in execution order" ] ]
        [ if List.isEmpty items then
            p [ class "muted" ] [ text "No update attempts recorded for this package." ]

          else
            ol [ class "history" ] (List.map (historyItem ctx) items)
        ]


historyItem : Ctx -> Group -> Html msg
historyItem ctx g =
    let
        phases =
            List.map .phase g.rows

        first =
            List.head g.rows

        running =
            Maybe.andThen .sessionStatus first == Just "running"

        mode =
            Maybe.andThen .sessionMode first |> Maybe.withDefault ""

        started =
            case first of
                Just r ->
                    case r.sessionStartedAt of
                        Just s ->
                            Just s

                        Nothing ->
                            Just r.phase.startedAt

                Nothing ->
                    Nothing

        total =
            List.sum (List.filterMap .durationMs phases)
    in
    li [ class "history-item" ]
        (div [ class "history-head" ]
            (List.concat
                [ [ View.badge (outcome phases running)
                  , text " "
                  , a [ class "mono", href (Format.sessionHref g.sessionId) ] [ text (Format.shortId g.sessionId) ]
                  ]
                , if mode /= "" then
                    [ text " ", span [ class "tag" ] [ text mode ] ]

                  else
                    []
                , [ text " "
                  , span [ class "muted small" ]
                        (View.ts ctx.now started
                            :: (if total > 0 then
                                    [ text (" · " ++ Format.durationMs total) ]

                                else
                                    []
                               )
                        )
                  ]
                ]
            )
            :: View.pchips ctx.now phases
            :: List.map (View.phaseError ctx.now False) (List.filter (\p -> p.status == "failed") phases)
        )



-- FAILURE LOGS


logsPanel : Ctx -> Data -> String -> Html msg
logsPanel ctx d label =
    panel "Failure logs"
        [ a [ class "small", href (Format.href "/failures" [ ( "q", d.attrPath ), ( "repo", label ) ]) ] [ text "Search failures" ] ]
        (if List.isEmpty d.logs then
            [ p [ class "muted" ] [ text "No failure logs recorded." ] ]

         else
            List.map (View.logEntry ctx.now False) d.logs
        )



-- AUTOFIX


autofixPanel : Ctx -> Data -> Html msg
autofixPanel ctx d =
    panel "Autofix"
        [ span [ class "muted small" ] [ text "LLM fix attempts (not scoped by repository)" ] ]
        (List.map (autofixItem ctx d.attempts) d.queue)


autofixItem : Ctx -> List Api.AutofixAttempt -> Api.AutofixQueue -> Html msg
autofixItem ctx allAttempts q =
    let
        attempts =
            List.filter (\a -> a.queueId == q.id) allAttempts
    in
    div [ class "autofix-item" ]
        (List.concat
            [ [ div [ class "history-head" ]
                    [ View.statusBadge q.status
                    , text " "
                    , span [] [ code [] [ text q.errorType ], text " in ", text (Format.humanize q.failedPhase) ]
                    , text " "
                    , span [ class "muted small" ]
                        (List.concat
                            [ [ text ("attempts " ++ String.fromInt q.attempts ++ " / " ++ String.fromInt q.maxAttempts ++ " · queued ")
                              , View.ts ctx.now (Just q.createdAt)
                              , text " · updated "
                              , View.ts ctx.now (Just q.updatedAt)
                              ]
                            , if q.fixedAt /= Nothing then
                                [ text " · fixed ", View.ts ctx.now q.fixedAt ]

                              else
                                []
                            , [ text " · session "
                              , a [ class "mono", href (Format.sessionHref q.sessionId) ] [ text (Format.shortId q.sessionId) ]
                              ]
                            ]
                        )
                    ]
              ]
            , if List.isEmpty attempts then
                []

              else
                [ div [ class "table-wrap" ]
                    [ table [ class "table compact" ]
                        [ thead []
                            [ tr []
                                [ th [ scope "col", class "num" ] [ text "#" ]
                                , th [ scope "col" ] [ text "Outcome" ]
                                , th [ scope "col" ] [ text "Started" ]
                                , th [ scope "col" ] [ text "Duration" ]
                                , th [ scope "col" ] [ text "Tokens" ]
                                , th [ scope "col" ] [ text "Build" ]
                                , th [ scope "col" ] [ text "Error" ]
                                ]
                            ]
                        , tbody [] (List.map (attemptRow ctx) attempts)
                        ]
                    ]
                ]
            , case nonEmpty q.artifactsPath of
                Just path ->
                    [ p [ class "small muted" ] [ text "Artifacts: ", code [] [ text path ] ] ]

                Nothing ->
                    []
            ]
        )


attemptRow : Ctx -> Api.AutofixAttempt -> Html msg
attemptRow ctx a =
    let
        duration =
            case a.completedAt of
                Just _ ->
                    Format.elapsedMs ctx.now a.startedAt a.completedAt
                        |> Maybe.map Format.durationMs
                        |> Maybe.withDefault Format.dash

                Nothing ->
                    Format.dash

        tokens =
            case ( a.promptTokens, a.completionTokens ) of
                ( Nothing, Nothing ) ->
                    Format.dash

                ( pt, ct ) ->
                    Format.thousands (Maybe.withDefault 0 pt)
                        ++ " in · "
                        ++ Format.thousands (Maybe.withDefault 0 ct)
                        ++ " out"

        build =
            case a.buildSuccess of
                Just True ->
                    "passed"

                Just False ->
                    "failed"

                Nothing ->
                    Format.dash
    in
    tr []
        [ td [ class "num" ] [ text (String.fromInt a.attemptNumber) ]
        , td []
            (View.statusBadge a.status
                :: (if a.changesApplied then
                        [ text " ", span [ class "tag" ] [ text "applied" ] ]

                    else
                        []
                   )
            )
        , td [] [ View.ts ctx.now (Just a.startedAt) ]
        , td [] [ text duration ]
        , td [ class "small" ] [ text tokens ]
        , td [] [ text build ]
        , td []
            [ case nonEmpty a.errorMessage of
                Just err ->
                    div [ class "small clip", title err ] [ text err ]

                Nothing ->
                    span [ class "muted" ] [ text Format.dash ]
            ]
        ]
