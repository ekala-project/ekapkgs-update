module Ctx exposing (Ctx, Page, field, query, queryInt)

{-| What every page view gets besides its data.
-}

import Dict exposing (Dict)
import Html exposing (Html)
import Msg exposing (Msg)
import Time


type alias Ctx =
    { now : Time.Posix

    -- Path of the current URL, e.g. `/packages`.
    , path : String

    -- Decoded query parameters of the current URL.
    , query : Dict String String

    -- Unsubmitted filter form values, by field name.
    , drafts : Dict String String
    }


{-| A rendered page: its `<title>` (without the site suffix) and content.
-}
type alias Page =
    { title : String
    , body : List (Html Msg)
    }


{-| A query parameter, `""` when absent.
-}
query : Ctx -> String -> String
query ctx key =
    Dict.get key ctx.query |> Maybe.withDefault ""


{-| An integer query parameter (e.g. `page`), with a default.
-}
queryInt : Ctx -> String -> Int -> Int
queryInt ctx key default =
    Dict.get key ctx.query
        |> Maybe.andThen (String.trim >> String.toInt)
        |> Maybe.withDefault default


{-| The current value of a filter field: the draft while editing, else the
query parameter.
-}
field : Ctx -> String -> String
field ctx key =
    case Dict.get key ctx.drafts of
        Just v ->
            v

        Nothing ->
            query ctx key
