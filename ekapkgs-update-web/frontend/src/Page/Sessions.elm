module Page.Sessions exposing (Data, decoder, view)

{-| The session list (`/sessions`), paginated and filtered server-side.
-}

import Api
import Ctx exposing (Ctx)
import Format
import Html exposing (..)
import Html.Attributes exposing (..)
import Json.Decode as D
import Msg exposing (Msg)
import Page.Session.Item as Item
import View


type alias Facet =
    { status : String
    , mode : String
    , count : Int
    }


type alias Data =
    { sessions : List Api.Session
    , total : Int
    , page : Int
    , perPage : Int
    , hiddenEmpty : Int
    , facets : List Facet
    }


decoder : D.Decoder Data
decoder =
    D.map6 Data
        (D.field "sessions" (D.list Api.session))
        (D.field "total" D.int)
        (D.field "page" D.int)
        (D.field "per_page" D.int)
        (D.field "hidden_empty" D.int)
        (D.field "facets"
            (D.list
                (D.map3 Facet
                    (D.field "status" D.string)
                    (D.field "mode" D.string)
                    (D.field "count" D.int)
                )
            )
        )


knownStatuses : List String
knownStatuses =
    [ "running", "completed", "failed", "cancelled" ]


view : Ctx -> Data -> Ctx.Page
view ctx data =
    let
        dbEmpty =
            List.isEmpty data.facets && data.hiddenEmpty == 0 && data.total == 0
    in
    { title = "Sessions"
    , body =
        [ div [ class "page-head" ]
            [ h1 [] [ text "Sessions" ]
            , p [ class "muted" ]
                [ text "One session per "
                , code [] [ text "run" ]
                , text " invocation, and one per "
                , code [] [ text "watch" ]
                , text " poll that dispatched updates."
                ]
            ]
        ]
            ++ (if dbEmpty then
                    [ View.emptyState "No sessions yet"
                        [ p []
                            [ text "Sessions are recorded by "
                            , code [] [ text "ekapkgs-update run" ]
                            , text " and by "
                            , code [] [ text "watch" ]
                            , text " polls that dispatch updates."
                            ]
                        ]
                    ]

                else
                    [ filterRow ctx data
                    , emptyNotice ctx data
                    ]
                        ++ (if List.isEmpty data.sessions then
                                [ View.emptyState "No matching sessions"
                                    [ p [] [ a [ href "/sessions" ] [ text "Show all sessions" ], text "." ] ]
                                ]

                            else
                                [ table ctx data
                                , View.pager ctx { page = data.page, perPage = data.perPage, total = data.total }
                                ]
                           )
               )
    }


filterRow : Ctx -> Data -> Html Msg
filterRow ctx data =
    let
        status =
            Ctx.query ctx "status"

        mode =
            Ctx.query ctx "mode"

        countWhere pred =
            data.facets |> List.filter pred |> List.map .count |> List.sum

        modeMatches f =
            mode == "" || f.mode == mode

        statusMatches f =
            status == "" || f.status == status

        extraStatuses =
            data.facets
                |> List.map .status
                |> List.filter (\s -> not (List.member s knownStatuses))
                |> unique

        statusChips =
            View.chip "All" (Just (countWhere modeMatches)) (View.queryHref ctx [ ( "status", "" ) ]) (status == "") "neutral"
                :: List.map
                    (\s ->
                        View.chip (Format.humanize s)
                            (Just (countWhere (\f -> f.status == s && modeMatches f)))
                            (View.queryHref ctx [ ( "status", s ) ])
                            (status == s)
                            (Format.tone s)
                    )
                    (knownStatuses ++ extraStatuses)

        modeChips =
            View.chip "All modes" Nothing (View.queryHref ctx [ ( "mode", "" ) ]) (mode == "") "neutral"
                :: List.map
                    (\m ->
                        View.chip m
                            (Just (countWhere (\f -> f.mode == m && statusMatches f)))
                            (View.queryHref ctx [ ( "mode", m ) ])
                            (mode == m)
                            "neutral"
                    )
                    [ "run", "watch" ]
    in
    div [ class "filter-row" ]
        ([ View.chips "Filter by status" statusChips
         , View.chips "Filter by mode" modeChips
         ]
            ++ (if status /= "" || mode /= "" then
                    [ a [ class "small", href "/sessions" ] [ text "Clear filters" ] ]

                else
                    []
               )
        )


emptyNotice : Ctx -> Data -> Html msg
emptyNotice ctx data =
    let
        showEmpty =
            Ctx.query ctx "show_empty" /= ""

        toggle label =
            a
                [ href
                    (View.queryHref ctx
                        [ ( "show_empty"
                          , if showEmpty then
                                ""

                            else
                                "1"
                          )
                        ]
                    )
                ]
                [ text label ]
    in
    if data.hiddenEmpty == 1 then
        p [ class "small muted" ] [ text "1 finished watch session that attempted nothing is hidden. ", toggle "Show it", text "." ]

    else if data.hiddenEmpty > 1 then
        p [ class "small muted" ]
            [ text (Format.thousands data.hiddenEmpty ++ " finished watch sessions that attempted nothing are hidden. ")
            , toggle "Show them"
            , text "."
            ]

    else if showEmpty then
        p [ class "small muted" ] [ text "Showing empty watch sessions. ", toggle "Hide them", text "." ]

    else
        text ""


table : Ctx -> Data -> Html msg
table ctx data =
    let
        num label =
            th [ class "num", scope "col" ] [ text label ]
    in
    div [ class "table-wrap" ]
        [ Html.table [ class "table" ]
            [ thead []
                [ tr []
                    [ th [ scope "col" ] [ text "Session" ]
                    , th [ scope "col" ] [ text "Status" ]
                    , th [ scope "col" ] [ text "Started" ]
                    , th [ scope "col" ] [ text "Duration" ]
                    , num "Evaluated"
                    , num "Attempted"
                    , num "Succeeded"
                    , num "Failed"
                    , num "Skipped"
                    , th [ scope "col" ] [ text "Progress" ]
                    ]
                ]
            , tbody [] (List.map (row ctx << Item.item ctx.now) data.sessions)
            ]
        ]


row : Ctx -> Item.Item -> Html msg
row ctx i =
    let
        s =
            i.session

        num n =
            td [ class "num" ] [ text (Format.thousands n) ]
    in
    tr
        (if i.isRunning then
            [ class "row-active" ]

         else
            []
        )
        [ td []
            [ a [ class "mono", href (Format.sessionHref s.id) ] [ text (Format.shortId s.id) ]
            , text " "
            , span [ class "tag" ] [ text s.mode ]
            ]
        , td [] [ View.badge i.badge ]
        , td [] [ View.ts ctx.now (Just s.startedAt) ]
        , td [] [ text i.duration ]
        , num s.evaluated
        , num s.attempted
        , num s.succeeded
        , num s.failed
        , num s.skipped
        , td [ class "progress-cell" ]
            (if s.attempted > 0 then
                [ Item.progressBar i
                , span [ class "small muted" ]
                    [ text
                        (if i.isRunning then
                            i.progressText

                         else
                            i.successRate ++ " success"
                        )
                    ]
                ]

             else
                [ span [ class "muted small" ] [ text "nothing attempted" ] ]
            )
        ]


unique : List String -> List String
unique =
    List.foldl
        (\x acc ->
            if List.member x acc then
                acc

            else
                acc ++ [ x ]
        )
        []
