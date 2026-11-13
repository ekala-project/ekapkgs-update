module View exposing
    ( Bar
    , Chip
    , Seg
    , badge
    , chip
    , chips
    , emptyState
    , filterActions
    , filters
    , hbars
    , logEntry
    , packageLink
    , pageHead
    , pager
    , pchips
    , phaseError
    , queryHref
    , rankedBars
    , segbar
    , selectField
    , statusBadge
    , textField
    , ts
    , ver
    )

{-| Shared rendering helpers (the markup the stylesheet expects).
-}

import Api
import Ctx exposing (Ctx)
import Dict
import Format
import Html exposing (..)
import Html.Attributes exposing (..)
import Html.Events exposing (onInput, onSubmit)
import Msg exposing (Msg(..))
import Route
import Time



-- VALUES


{-| A timestamp: relative text, exact UTC time on hover, `—` when missing.
-}
ts : Time.Posix -> Maybe String -> Html msg
ts now raw =
    case Maybe.map String.trim raw of
        Just r ->
            if r == "" then
                span [ class "muted" ] [ text Format.dash ]

            else
                case Format.parseTs r of
                    Just t ->
                        node "time" [ attribute "datetime" r, title (Format.absolute t) ] [ text (Format.relative now t) ]

                    Nothing ->
                        -- Unparseable: show the raw value rather than hide it.
                        node "time" [ title r ] [ text r ]

        Nothing ->
            span [ class "muted" ] [ text Format.dash ]


badge : Format.Badge -> Html msg
badge b =
    span [ class ("badge tone-" ++ b.tone) ] [ span [ class "dot", attribute "aria-hidden" "true" ] [], text b.label ]


statusBadge : String -> Html msg
statusBadge =
    Format.statusBadge >> badge


{-| A version, shortened, with the full value on hover.
-}
ver : Maybe String -> Html msg
ver v =
    case v of
        Just full ->
            if full == "" then
                span [ class "muted" ] [ text Format.dash ]

            else
                code [ class "ver", title full ] [ text (Format.shortVersion full) ]

        Nothing ->
            span [ class "muted" ] [ text Format.dash ]



-- SEGMENTED BARS


{-| One segment: CSS class (`seg-good`, …), value and what it counts.
-}
type alias Seg =
    { class : String
    , value : Int
    , name : String
    }


{-| A thin segmented bar; zero-valued segments are dropped.
-}
segbar : String -> List Seg -> Html msg
segbar label segments =
    case List.filter (\s -> s.value > 0) segments of
        [] ->
            div [ class "segbar segbar-empty", attribute "role" "img", attribute "aria-label" "No data" ] []

        kept ->
            div [ class "segbar", attribute "role" "img", attribute "aria-label" label ]
                (List.map
                    (\s ->
                        span
                            [ class ("seg " ++ s.class)
                            , style "flex-grow" (String.fromInt s.value)
                            , attribute "data-tip" (Format.thousands s.value ++ " " ++ s.name)
                            , tabindex 0
                            ]
                            []
                    )
                    kept
                )



-- CHIPS AND PAGINATION


type alias Chip =
    { label : String
    , count : Maybe Int
    , href : String
    , active : Bool
    , tone : String
    }


chip : String -> Maybe Int -> String -> Bool -> String -> Chip
chip label count href_ active tone_ =
    { label = label, count = count, href = href_, active = active, tone = tone_ }


chips : String -> List Chip -> Html msg
chips label items =
    nav [ class "chips", attribute "aria-label" label ]
        (List.map
            (\c ->
                a
                    ([ class ("chip tone-" ++ c.tone), href c.href ]
                        ++ (if c.active then
                                [ attribute "aria-current" "true" ]

                            else
                                []
                           )
                    )
                    (List.concat
                        [ if c.tone /= "neutral" then
                            [ span [ class "dot", attribute "aria-hidden" "true" ] [] ]

                          else
                            []
                        , [ text c.label ]
                        , case c.count of
                            Just n ->
                                [ span [ class "chip-count" ] [ text (Format.thousands n) ] ]

                            Nothing ->
                                []
                        ]
                    )
            )
            items
        )


{-| The current page's URL with some parameters replaced (`""` removes
one). `page` is always reset.
-}
queryHref : Ctx -> List ( String, String ) -> String
queryHref ctx overrides =
    let
        params =
            List.foldl (\( k, v ) acc -> Dict.insert k v acc) (Dict.remove "page" ctx.query) overrides
    in
    Route.withQuery ctx.path params


{-| Previous/next links around `page`: the effective page the server
reported (it clamps `?page=` to the last page), or the client's own
clamped page for lists paginated in the browser.
-}
pager : Ctx -> { page : Int, perPage : Int, total : Int } -> Html msg
pager ctx args =
    let
        perPage =
            args.perPage

        total =
            args.total

        pages =
            Basics.max 1 ((total + perPage - 1) // perPage)

        page =
            clamp 1 pages args.page

        link p =
            Route.withQuery ctx.path (Dict.insert "page" (String.fromInt p) ctx.query)

        first =
            if total == 0 then
                0

            else
                (page - 1) * perPage + 1

        last =
            Basics.min total (page * perPage)
    in
    if pages > 1 then
        nav [ class "pager", attribute "aria-label" "Pagination" ]
            [ if page > 1 then
                a [ class "button", href (link (page - 1)), rel "prev" ] [ text "← Previous" ]

              else
                span [ class "button", attribute "aria-disabled" "true" ] [ text "← Previous" ]
            , span [ class "pager-info" ]
                [ text
                    ("Page "
                        ++ String.fromInt page
                        ++ " of "
                        ++ String.fromInt pages
                        ++ " · "
                        ++ Format.thousands first
                        ++ "–"
                        ++ Format.thousands last
                        ++ " of "
                        ++ Format.thousands total
                    )
                ]
            , if page < pages then
                a [ class "button", href (link (page + 1)), rel "next" ] [ text "Next →" ]

              else
                span [ class "button", attribute "aria-disabled" "true" ] [ text "Next →" ]
            ]

    else if total > 0 then
        p [ class "pager-info" ]
            [ text
                (Format.thousands total
                    ++ (if total == 1 then
                            " result"

                        else
                            " results"
                       )
                )
            ]

    else
        text ""



-- PHASES AND LOGS


{-| Phases as small chips (label, tone, duration).
-}
pchips : Time.Posix -> List Api.Phase -> Html msg
pchips now list =
    span [ class "pchips" ]
        (List.map
            (\ph ->
                let
                    duration =
                        case ph.durationMs of
                            Just ms ->
                                Format.durationMs ms

                            Nothing ->
                                if ph.status == "running" then
                                    Format.elapsedMs now ph.startedAt Nothing
                                        |> Maybe.map Format.durationMs
                                        |> Maybe.withDefault ""

                                else
                                    ""

                    tip =
                        String.join " · "
                            (List.filter ((/=) "")
                                [ ph.phase
                                , Format.humanize ph.status
                                , duration
                                , Maybe.withDefault "" ph.errorType
                                ]
                            )
                in
                span [ class ("pchip tone-" ++ Format.tone ph.status), title tip ]
                    (text (Format.phaseShort ph.phase)
                        :: (if duration == "" then
                                []

                            else
                                [ span [ class "pchip-time" ] [ text duration ] ]
                           )
                    )
            )
            list
        )


packageLink : String -> String -> List (Html msg)
packageLink repoName attrPath =
    a [ href (Format.packageHref repoName attrPath), class "attr" ] [ text attrPath ]
        :: (if repoName == "" then
                []

            else
                [ text " ", span [ class "muted" ] [ text ("(" ++ repoName ++ ")") ] ]
           )


{-| A failed phase's error as collapsible details. `showPackage` adds a
link to the package (for lists spanning packages).
-}
phaseError : Time.Posix -> Bool -> Api.Phase -> Html msg
phaseError now showPackage ph =
    let
        details =
            ph.errorDetails |> Maybe.map Format.prettyJson |> Maybe.withDefault ""
    in
    details_ [ class "error-block" ]
        [ summary []
            (List.concat
                [ [ badge (Format.badge (Maybe.withDefault "error" ph.errorType) "critical"), text " " ]
                , if showPackage then
                    packageLink ph.repoName ph.attrPath ++ [ text " " ]

                  else
                    []
                , [ span [ class "muted" ]
                        [ text ("in " ++ ph.phase ++ " · ")
                        , ts now (Just (Maybe.withDefault ph.startedAt ph.completedAt))
                        ]
                  ]
                ]
            )
        , if details == "" then
            p [ class "muted" ] [ text "No error details were recorded." ]

          else
            pre [ class "code" ] [ text details ]
        , case ph.artifactsPath of
            Just path ->
                if path == "" then
                    text ""

                else
                    p [ class "small" ] [ text "Preserved artifacts: ", code [] [ text path ] ]

            Nothing ->
                text ""
        ]


details_ : List (Attribute msg) -> List (Html msg) -> Html msg
details_ =
    node "details"


{-| A failure log as collapsible details, with its last non-empty line as
the summary.
-}
logEntry : Time.Posix -> Bool -> Api.Log -> Html msg
logEntry now showPackage l =
    let
        lastLine =
            l.logTail
                |> String.lines
                |> List.map String.trim
                |> List.filter ((/=) "")
                |> List.reverse
                |> List.head
                |> Maybe.map (Format.truncate 180)
                |> Maybe.withDefault ""

        nonEmpty v =
            case v of
                Just s ->
                    s /= ""

                Nothing ->
                    False
    in
    details_ [ class "log-entry" ]
        [ summary []
            (List.concat
                [ if showPackage then
                    packageLink l.repoName l.attrPath ++ [ text " " ]

                  else
                    []
                , if nonEmpty l.errorType then
                    [ badge (Format.badge (Maybe.withDefault "" l.errorType) "critical"), text " " ]

                  else
                    []
                , if nonEmpty l.phase then
                    [ span [ class "tag" ] [ text (Maybe.withDefault "" l.phase) ], text " " ]

                  else
                    []
                , [ span [ class "muted" ] [ ver l.oldVersion, text " → ", ver l.newVersion, text " · ", ts now (Just l.timestamp) ] ]
                , if lastLine == "" then
                    []

                  else
                    [ text " ", span [ class "log-summary" ] [ text lastLine ] ]
                ]
            )
        , div [ class "log-meta small" ]
            (List.concat
                [ [ span [] [ text "Key ", code [] [ text l.drvPath ] ] ]
                , case l.sessionId of
                    Just id ->
                        [ text " · ", a [ href (Format.sessionHref id) ] [ text ("session " ++ Format.shortId id) ] ]

                    Nothing ->
                        []
                , [ text (" · " ++ Format.bytes l.logLen) ]
                , if l.logLen > 6000 then
                    [ text " · showing the last 6,000 characters" ]

                  else
                    []
                ]
            )
        , pre [ class "code log" ] [ text l.logTail ]
        ]



-- RANKED BARS


{-| A horizontal bar in a ranked list.
-}
type alias Bar =
    { label : String
    , value : Int
    , href : Maybe String
    , badge : Maybe Format.Badge
    }


{-| Bars from (label, value), in the given order, without links or badges.
-}
rankedBars : List ( String, Int ) -> List Bar
rankedBars =
    List.map (\( label, value ) -> { label = label, value = value, href = Nothing, badge = Nothing })


{-| Ranked bars (one hue; width relative to the largest) with a
"Table view" twin. `unit` heads the table's label column.
-}
hbars : String -> List Bar -> Html msg
hbars unit bars =
    let
        maxValue =
            List.map .value bars |> List.maximum |> Maybe.withDefault 0

        total =
            List.map .value bars |> List.sum

        share b =
            Format.pct (Format.ratioPct b.value total)
    in
    div []
        [ ol [ class "hbars" ]
            (List.map
                (\b ->
                    li [ class "hbar-row" ]
                        [ span [ class "hbar-label" ]
                            [ case ( b.badge, b.href ) of
                                ( Just bd, _ ) ->
                                    badge bd

                                ( Nothing, Just h ) ->
                                    a [ href h ] [ text b.label ]

                                ( Nothing, Nothing ) ->
                                    text b.label
                            ]
                        , span [ class "hbar-track" ]
                            [ span
                                [ class "hbar-fill"
                                , style "width" (Format.cssPct (toFloat b.value) (toFloat maxValue))
                                , attribute "data-tip" (b.label ++ ": " ++ Format.thousands b.value ++ " (" ++ share b ++ ")")
                                , tabindex 0
                                ]
                                []
                            ]
                        , span [ class "hbar-value" ] [ text (Format.thousands b.value) ]
                        ]
                )
                bars
            )
        , details_ [ class "table-view" ]
            [ summary [] [ text "Table view" ]
            , table [ class "table compact" ]
                [ thead [] [ tr [] [ th [ scope "col" ] [ text unit ], th [ scope "col", class "num" ] [ text "Count" ], th [ scope "col", class "num" ] [ text "Share" ] ] ]
                , tbody []
                    (List.map
                        (\b -> tr [] [ td [] [ text b.label ], td [ class "num" ] [ text (Format.thousands b.value) ], td [ class "num" ] [ text (share b) ] ])
                        bars
                    )
                ]
            ]
        ]



-- PAGE STRUCTURE


pageHead : String -> String -> Html msg
pageHead heading lede =
    div [ class "page-head" ]
        [ h1 [] [ text heading ]
        , if lede == "" then
            text ""

          else
            p [ class "muted" ] [ text lede ]
        ]


emptyState : String -> List (Html msg) -> Html msg
emptyState heading content =
    div [ class "empty-state" ] (h2 [] [ text heading ] :: content)



-- FILTER FORMS


{-| A filter form. Submitting navigates to `ctx.path` with the named
`fields` taken from the form (other query parameters are kept, `page`
is reset).
-}
filters : Ctx -> List String -> List (Html Msg) -> Html Msg
filters ctx fields content =
    Html.form [ class "filters", attribute "role" "search", onSubmit (Submit ctx.path fields) ] content


textField : Ctx -> { key : String, label : String, placeholder : String } -> Html Msg
textField ctx f =
    label [ class "field grow" ]
        [ span [] [ text f.label ]
        , input [ type_ "search", name f.key, value (Ctx.field ctx f.key), placeholder f.placeholder, onInput (Draft f.key) ] []
        ]


{-| A `<select>`; `options` are (value, label).
-}
selectField : Ctx -> { key : String, label : String } -> List ( String, String ) -> Html Msg
selectField ctx f options =
    let
        current =
            Ctx.field ctx f.key
    in
    label [ class "field" ]
        [ span [] [ text f.label ]
        , select [ name f.key, onInput (Draft f.key) ]
            (List.map (\( v, l ) -> option [ value v, selected (v == current) ] [ text l ]) options)
        ]


{-| "Apply", plus "Clear" (linking to `clearHref`) when filters are set.
-}
filterActions : Bool -> String -> Html msg
filterActions hasFilters clearHref =
    div [ class "field-actions" ]
        (button [ class "button primary", type_ "submit" ] [ text "Apply" ]
            :: (if hasFilters then
                    [ a [ class "button", href clearHref ] [ text "Clear" ] ]

                else
                    []
               )
        )
