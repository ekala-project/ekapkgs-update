module Page.Failures exposing (Data, decoder, view)

{-| Recorded update failures (`/failures`), filtered and paginated
server-side.
-}

import Api
import Ctx exposing (Ctx)
import Format
import Html exposing (..)
import Html.Attributes exposing (..)
import Json.Decode as D
import View


type alias Facets =
    { repos : List String
    , phases : List String
    , errorTypes : List String
    }


type alias Data =
    { logs : List Api.Log
    , total : Int
    , perPage : Int
    , facets : Facets
    }


decoder : D.Decoder Data
decoder =
    D.map4 Data
        (D.field "logs" (D.list Api.log))
        (D.field "total" D.int)
        (D.field "per_page" D.int)
        (D.field "facets"
            (D.map3 Facets
                (D.field "repos" (D.list D.string))
                (D.field "phases" (D.list D.string))
                (D.field "error_types" (D.list D.string))
            )
        )


filterKeys : List String
filterKeys =
    [ "q", "repo", "phase", "error_type" ]


view : Ctx -> Data -> Ctx.Page
view ctx data =
    let
        hasFilters =
            List.any (\k -> Ctx.query ctx k /= "") filterKeys
    in
    { title = "Failures"
    , body =
        [ View.pageHead "Failures" "Recorded update failures with the tail of each build or error log."
        , View.filters ctx
            filterKeys
            [ View.textField ctx { key = "q", label = "Attribute", placeholder = "attribute path contains…" }
            , View.selectField ctx
                { key = "repo", label = "Repository" }
                (( "", "All repos" ) :: List.map (\r -> ( Format.repoLabel r, Format.repoLabel r )) data.facets.repos)
            , View.selectField ctx
                { key = "phase", label = "Phase" }
                (( "", "Any phase" ) :: List.map (\p -> ( p, Format.phaseShort p ++ " (" ++ Format.humanize p ++ ")" )) data.facets.phases)
            , View.selectField ctx
                { key = "error_type", label = "Error type" }
                (( "", "Any error type" ) :: List.map (\t -> ( t, t )) data.facets.errorTypes)
            , View.filterActions hasFilters "/failures"
            ]
        ]
            ++ (if not hasFilters && data.total == 0 then
                    [ View.emptyState "No failures recorded" [ p [] [ text "Failed update attempts appear here with their logs." ] ] ]

                else if List.isEmpty data.logs then
                    [ View.emptyState "No matching failures" [ p [] [ a [ href "/failures" ] [ text "Show all failures" ], text "." ] ] ]

                else
                    [ div [ class "log-list" ] (List.map (View.logEntry ctx.now True) data.logs)
                    , View.pager ctx { perPage = data.perPage, total = data.total }
                    ]
               )
    }
