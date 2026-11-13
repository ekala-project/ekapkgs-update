module Msg exposing (LiveState, Msg(..))

{-| Messages, in their own module so page views (which emit them) and
`Main` (which handles them) do not import each other.
-}

import Api
import Browser
import Json.Decode as D
import Time
import Url


type alias LiveState =
    { state : String
    , label : String
    }


type Msg
    = LinkClicked Browser.UrlRequest
    | UrlChanged Url.Url
      -- Response to the request with this sequence number.
    | GotData Int (Result Api.Error D.Value)
    | Tick Time.Posix
    | Refresh
    | LiveStatus LiveState
    | LiveUpdate String
      -- Filter forms: edit a field, then submit the fields as the query
      -- string of a path.
    | Draft String String
    | Submit String (List String)
    | NoOp
