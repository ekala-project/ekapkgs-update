module Page.Packages exposing (Data, decoder, view)

{-| `/packages`: every tracked package, filterable and sortable.
-}

import Api
import Ctx exposing (Ctx)
import Format
import Html exposing (..)
import Html.Attributes exposing (..)
import Json.Decode as D
import Msg exposing (Msg)
import View


type alias Sort =
    { key : String
    , label : String
    }


type alias Data =
    { packages : List Api.Package
    , total : Int
    , allTotal : Int
    , page : Int
    , perPage : Int
    , sort : String
    , sorts : List Sort
    , statusCounts : Api.StatusCounts
    , repos : List String
    , upstreamTypes : List String
    , vulnerable : List Vulnerable
    , vulnerableTotal : Int
    }


{-| A package with high or critical vulnerabilities at its current version
(`VulnerablePackageRow`).
-}
type alias Vulnerable =
    { repoName : String
    , attrPath : String
    , pname : Maybe String
    , status : String
    , currentVersion : Maybe String
    , latestUpstreamVersion : Maybe String
    , prUrl : Maybe String
    , prNumber : Maybe Int
    , critical : Int
    , high : Int
    , fixedByUpdate : Int
    , maxScore : Maybe Float
    , topIds : List String
    , cveScannedAt : Maybe String
    }


vulnerableDecoder : D.Decoder Vulnerable
vulnerableDecoder =
    D.succeed Vulnerable
        |> Api.required "repo_name" D.string
        |> Api.required "attr_path" D.string
        |> Api.nullable "pname" D.string
        |> Api.required "status" D.string
        |> Api.nullable "current_version" D.string
        |> Api.nullable "latest_upstream_version" D.string
        |> Api.nullable "pr_url" D.string
        |> Api.nullable "pr_number" D.int
        |> Api.required "critical" D.int
        |> Api.required "high" D.int
        |> Api.required "fixed_by_update" D.int
        |> Api.nullable "max_score" D.float
        |> Api.required "top_ids" (D.list D.string)
        |> Api.nullable "cve_scanned_at" D.string


decoder : D.Decoder Data
decoder =
    D.succeed Data
        |> Api.required "packages" (D.list Api.package)
        |> Api.required "total" D.int
        |> Api.required "all_total" D.int
        |> Api.required "page" D.int
        |> Api.required "per_page" D.int
        |> Api.required "sort" D.string
        |> Api.required "sorts"
            (D.list
                (D.map2 Sort
                    (D.field "key" D.string)
                    (D.field "label" D.string)
                )
            )
        |> Api.required "status_counts" Api.statusCounts
        |> Api.required "repos" (D.list D.string)
        |> Api.required "upstream_types" (D.list D.string)
        |> Api.andMap (D.at [ "high_severity", "packages" ] (D.list vulnerableDecoder))
        |> Api.andMap (D.at [ "high_severity", "total" ] D.int)


{-| Status chips after "All", in display order.
-}
statusChips : List String
statusChips =
    [ "up_to_date", "outdated", "backlog", "proposed", "failed", "upstream_error", "pending" ]


statusCount : Api.StatusCounts -> String -> Int
statusCount c status =
    case status of
        "up_to_date" ->
            c.upToDate

        "outdated" ->
            c.outdated

        "backlog" ->
            c.backlog

        "proposed" ->
            c.proposed

        "failed" ->
            c.failed

        "upstream_error" ->
            c.upstreamError

        "pending" ->
            c.pending

        _ ->
            0


view : Ctx -> Data -> Ctx.Page
view ctx data =
    let
        currentStatus =
            Ctx.query ctx "status"

        hasFilters =
            List.any (\k -> Ctx.query ctx k /= "") [ "q", "repo", "status", "upstream" ]

        showRepo =
            List.length data.repos > 1
    in
    { title = "Packages"
    , body =
        [ View.pageHead "Packages" "Every package ekapkgs-update has compared against upstream."
        , filterForm ctx data hasFilters
        , View.chips "Filter by status"
            (View.chip "All" (Just data.statusCounts.total) (View.queryHref ctx [ ( "status", "" ) ]) (currentStatus == "") "neutral"
                :: List.map (statusChip ctx data currentStatus) statusChips
            )
        , if data.vulnerableTotal > 0 then
            vulnerablePanel ctx showRepo data

          else
            text ""
        , if data.allTotal == 0 then
            View.emptyState "No packages tracked yet"
                [ p []
                    [ text "Run "
                    , code [] [ text "ekapkgs-update run" ]
                    , text " or start "
                    , code [] [ text "ekapkgs-update watch" ]
                    , text "; packages appear here as soon as they are checked."
                    ]
                ]

          else if List.isEmpty data.packages then
            View.emptyState "No matching packages"
                [ p [] [ text "Nothing matches these filters. ", a [ href "/packages" ] [ text "Show all packages" ], text "." ] ]

          else
            div [ class "table-wrap" ]
                [ table [ class "table" ]
                    [ thead []
                        [ tr []
                            (List.concat
                                [ if showRepo then
                                    [ th [ scope "col" ] [ text "Repo" ] ]

                                  else
                                    []
                                , [ th [ scope "col" ] [ sortLink ctx data "attr" "Attribute" ]
                                  , th [ scope "col" ] [ text "Current → latest" ]
                                  , th [ scope "col" ] [ sortLink ctx data "status" "Status" ]
                                  , th [ scope "col" ] [ text "PR" ]
                                  , th [ scope "col" ] [ sortLink ctx data "checked" "Last checked" ]
                                  , th [ scope "col", class "num" ] [ sortLink ctx data "rebuilds" "Rebuilds" ]
                                  , th [ scope "col", class "num" ] [ sortLink ctx data "failures" "Failures" ]
                                  ]
                                ]
                            )
                        ]
                    , tbody [] (List.map (row ctx showRepo) data.packages)
                    ]
                ]
        , View.pager ctx { page = data.page, perPage = data.perPage, total = data.total }
        ]
    }


statusChip : Ctx -> Data -> String -> String -> View.Chip
statusChip ctx data currentStatus s =
    View.chip
        (if s == "backlog" then
            "backlog"

         else
            Format.humanize s
        )
        (Just (statusCount data.statusCounts s))
        (View.queryHref ctx [ ( "status", s ) ])
        (currentStatus == s)
        (if s == "backlog" then
            "warn"

         else
            Format.tone s
        )


filterForm : Ctx -> Data -> Bool -> Html Msg
filterForm ctx data hasFilters =
    View.filters ctx
        [ "q", "repo", "upstream", "sort" ]
        [ View.textField ctx { key = "q", label = "Search", placeholder = "attribute path or pname" }
        , View.selectField ctx
            { key = "repo", label = "Repository" }
            (( "", "All repos" ) :: List.map (\r -> ( Format.repoLabel r, Format.repoLabel r )) data.repos)
        , View.selectField ctx
            { key = "upstream", label = "Upstream" }
            (( "", "Any upstream" ) :: List.map (\u -> ( u, u )) data.upstreamTypes)
        , View.selectField ctx
            { key = "sort", label = "Sort by" }
            (List.map (\s -> ( s.key, s.label )) data.sorts)
        , View.filterActions hasFilters "/packages"
        ]


{-| A column header that sorts by `key` (the default `attr` drops the
parameter).
-}
sortLink : Ctx -> Data -> String -> String -> Html msg
sortLink ctx data key label =
    a
        (href
            (View.queryHref ctx
                [ ( "sort"
                  , if key == "attr" then
                        ""

                    else
                        key
                  )
                ]
            )
            :: (if data.sort == key then
                    [ attribute "aria-current" "true" ]

                else
                    []
               )
        )
        [ text label ]


row : Ctx -> Bool -> Api.Package -> Html msg
row ctx showRepo p =
    let
        latestDiffers =
            p.latestUpstreamVersion /= Nothing && p.latestUpstreamVersion /= p.currentVersion

        proposedDiffers =
            p.proposedVersion /= Nothing && p.proposedVersion /= p.latestUpstreamVersion
    in
    tr []
        (List.concat
            [ if showRepo then
                [ td [ class "small" ] [ text (Format.repoLabel p.repoName) ] ]

              else
                []
            , [ td [] (attrCell p)
              , td [ class "versions" ]
                    (List.concat
                        [ [ View.ver p.currentVersion ]
                        , if latestDiffers then
                            [ text " → ", View.ver p.latestUpstreamVersion ]

                          else
                            []
                        , if proposedDiffers then
                            [ span [ class "muted small" ] [ text " (proposed ", View.ver p.proposedVersion, text ")" ] ]

                          else
                            []
                        ]
                    )
              , td []
                    (View.badge (Format.packageBadge p.status p.isBacklog)
                        :: (case p.lastError of
                                Just err ->
                                    if err /= "" then
                                        [ div [ class "small muted clip", title err ] [ text (Format.truncate 160 err) ] ]

                                    else
                                        []

                                Nothing ->
                                    []
                           )
                    )
              , td [] [ prLink p.prUrl p.prNumber p.status ]
              , td []
                    [ View.ts ctx.now
                        (case p.lastChecked of
                            Just c ->
                                Just c

                            Nothing ->
                                p.lastAttempted
                        )
                    ]
              , td [ class "num" ]
                    [ case p.rebuildCount of
                        Just n ->
                            text (Format.thousands n)

                        Nothing ->
                            span [ class "muted" ] [ text Format.dash ]
                    ]
              , td [ class "num" ]
                    [ if p.failureCount > 0 then
                        span [ class "tone-text-critical" ] [ text (Format.thousands p.failureCount) ]

                      else
                        span [ class "muted" ] [ text "0" ]
                    ]
              ]
            ]
        )


attrCell : Api.Package -> List (Html msg)
attrCell p =
    List.concat
        [ [ a [ class "attr", href (Format.packageHref p.repoName p.attrPath) ] [ text p.attrPath ] ]
        , case p.pname of
            Just pname ->
                if pname /= "" && pname /= p.attrPath then
                    [ text " ", span [ class "muted small" ] [ text pname ] ]

                else
                    []

            Nothing ->
                []
        , case p.upstreamType of
            Just kind ->
                [ text " ", span [ class "tag" ] [ text kind ] ]

            Nothing ->
                []
        ]


prLink : Maybe String -> Maybe Int -> String -> Html msg
prLink prUrl prNumber status =
    case Format.safeUrl prUrl of
        Just url ->
            let
                state =
                    Format.prState status
            in
            span []
                (a [ href url, target "_blank", rel "noopener noreferrer", title (state ++ " PR") ]
                    [ text
                        (case prNumber of
                            Just n ->
                                "#" ++ String.fromInt n

                            Nothing ->
                                "PR"
                        )
                    ]
                    :: (if state == "open" then
                            []

                        else
                            [ text " ", span [ class "muted small" ] [ text state ] ]
                       )
                )

        Nothing ->
            span [ class "muted" ] [ text Format.dash ]



-- VULNERABILITIES


vulnerablePanel : Ctx -> Bool -> Data -> Html msg
vulnerablePanel ctx showRepo data =
    let
        shown =
            List.length data.vulnerable
    in
    section [ class "panel", attribute "aria-labelledby" "h-vulnerable" ]
        [ header [ class "panel-head" ]
            [ h2 [ id "h-vulnerable" ] [ text "Packages with high-severity CVEs" ]
            , span [ class "muted small" ]
                [ text
                    (if shown < data.vulnerableTotal then
                        "Top " ++ Format.thousands shown ++ " of " ++ Format.thousands data.vulnerableTotal

                     else
                        Format.thousands data.vulnerableTotal
                            ++ (if data.vulnerableTotal == 1 then
                                    " package"

                                else
                                    " packages"
                               )
                    )
                ]
            ]
        , p [ class "muted small" ]
            [ text "Known high or critical vulnerabilities (OSV.dev) at the packaged version. \"Fixed by update\" counts those not reported for the latest upstream release." ]
        , div [ class "table-wrap table-scroll" ]
            [ table [ class "table compact" ]
                [ thead []
                    [ tr []
                        (List.concat
                            [ if showRepo then
                                [ th [ scope "col" ] [ text "Repo" ] ]

                              else
                                []
                            , [ th [ scope "col" ] [ text "Attribute" ]
                              , th [ scope "col" ] [ text "Current → latest" ]
                              , th [ scope "col", class "num" ] [ text "Critical" ]
                              , th [ scope "col", class "num" ] [ text "High" ]
                              , th [ scope "col", class "num" ] [ text "Max CVSS" ]
                              , th [ scope "col" ] [ text "Most severe" ]
                              , th [ scope "col" ] [ text "Fixed by update" ]
                              , th [ scope "col" ] [ text "PR" ]
                              , th [ scope "col" ] [ text "Scanned" ]
                              ]
                            ]
                        )
                    ]
                , tbody [] (List.map (vulnerableRow ctx showRepo) data.vulnerable)
                ]
            ]
        ]


vulnerableRow : Ctx -> Bool -> Vulnerable -> Html msg
vulnerableRow ctx showRepo v =
    let
        total =
            v.critical + v.high

        hasUpdate =
            v.latestUpstreamVersion /= Nothing && v.latestUpstreamVersion /= v.currentVersion
    in
    tr []
        (List.concat
            [ if showRepo then
                [ td [ class "small" ] [ text (Format.repoLabel v.repoName) ] ]

              else
                []
            , [ td [] [ a [ class "attr", href (Format.packageHref v.repoName v.attrPath) ] [ text v.attrPath ] ]
              , td [ class "versions" ]
                    (View.ver v.currentVersion
                        :: (if hasUpdate then
                                [ text " → ", View.ver v.latestUpstreamVersion ]

                            else
                                []
                           )
                    )
              , td [ class "num" ] [ countCell "critical" v.critical ]
              , td [ class "num" ] [ countCell "serious" v.high ]
              , td [ class "num" ]
                    [ case v.maxScore of
                        Just score ->
                            text (Format.fixed1 score)

                        Nothing ->
                            span [ class "muted" ] [ text Format.dash ]
                    ]
              , td [ class "small" ]
                    (List.intersperse (text ", ")
                        (List.map
                            (\id -> a [ href (Format.osvUrl id), target "_blank", rel "noopener noreferrer" ] [ text id ])
                            v.topIds
                        )
                        ++ (if total > List.length v.topIds then
                                [ span [ class "muted" ] [ text (" +" ++ String.fromInt (total - List.length v.topIds)) ] ]

                            else
                                []
                           )
                    )
              , td []
                    [ if not hasUpdate then
                        span [ class "muted" ] [ text "no update" ]

                      else if v.fixedByUpdate == total then
                        span [ class "tone-text-good" ] [ text ("all " ++ String.fromInt total) ]

                      else if v.fixedByUpdate == 0 then
                        span [ class "tone-text-critical" ] [ text "none" ]

                      else
                        text (String.fromInt v.fixedByUpdate ++ " of " ++ String.fromInt total)
                    ]
              , td [] [ prLink v.prUrl v.prNumber v.status ]
              , td [] [ View.ts ctx.now v.cveScannedAt ]
              ]
            ]
        )


countCell : String -> Int -> Html msg
countCell t n =
    if n > 0 then
        span [ class ("tone-text-" ++ t) ] [ text (Format.thousands n) ]

    else
        span [ class "muted" ] [ text "0" ]
