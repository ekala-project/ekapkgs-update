module Route exposing (Route(..), apiUrl, fromUrl, parseQuery, samePage, withQuery)

{-| Client-side routes. They match the server's page routes
(`ekapkgs-update-web/src/routes/mod.rs`), which all return the app shell.
-}

import Dict exposing (Dict)
import Url exposing (Url)
import Url.Parser as P exposing ((</>))


type Route
    = Dashboard
    | Packages
    | Package String
    | Sessions
    | Session String
    | Failures
    | Watch
    | Analytics
    | NotFound


fromUrl : Url -> Route
fromUrl url =
    P.parse parser { url | query = Nothing, fragment = Nothing }
        |> Maybe.withDefault NotFound


parser : P.Parser (Route -> a) a
parser =
    P.oneOf
        [ P.map Dashboard P.top
        , P.map Packages (P.s "packages")
        , P.map (decode >> Package) (P.s "packages" </> P.string)
        , P.map Sessions (P.s "sessions")
        , P.map (decode >> Session) (P.s "sessions" </> P.string)
        , P.map Failures (P.s "failures")
        , P.map Watch (P.s "watch")
        , P.map Analytics (P.s "analytics")
        ]


decode : String -> String
decode s =
    Url.percentDecode s |> Maybe.withDefault s


{-| The API request backing a route, with the page's query string passed
through (endpoints ignore parameters they do not know).
-}
apiUrl : Route -> Maybe String -> Maybe String
apiUrl route rawQuery =
    let
        path =
            case route of
                Dashboard ->
                    Just "/api/dashboard"

                Packages ->
                    Just "/api/packages"

                Package attr ->
                    Just ("/api/packages/" ++ Url.percentEncode attr)

                Sessions ->
                    Just "/api/sessions"

                Session id ->
                    Just ("/api/sessions/" ++ Url.percentEncode id)

                Failures ->
                    Just "/api/failures"

                Watch ->
                    Just "/api/watch"

                Analytics ->
                    Just "/api/analytics"

                NotFound ->
                    Nothing
    in
    Maybe.map
        (\p ->
            case rawQuery of
                Just q ->
                    if q == "" then
                        p

                    else
                        p ++ "?" ++ q

                Nothing ->
                    p
        )
        path


{-| Whether two routes show the same thing (so data on screen may stay
while new data loads, e.g. after a filter change).
-}
samePage : Route -> Route -> Bool
samePage a b =
    case ( a, b ) of
        ( Package x, Package y ) ->
            x == y

        ( Session x, Session y ) ->
            x == y

        _ ->
            a == b


{-| Parse `a=1&b=x+y` (form encoding: `+` is a space).
-}
parseQuery : Maybe String -> Dict String String
parseQuery raw =
    let
        dec s =
            String.replace "+" " " s |> Url.percentDecode |> Maybe.withDefault s

        pair part =
            case String.split "=" part of
                [] ->
                    Nothing

                [ "" ] ->
                    Nothing

                key :: rest ->
                    Just ( dec key, dec (String.join "=" rest) )
    in
    raw
        |> Maybe.withDefault ""
        |> String.split "&"
        |> List.filterMap pair
        |> Dict.fromList


{-| `path?query` from pairs, skipping empty values.
-}
withQuery : String -> Dict String String -> String
withQuery path params =
    let
        pairs =
            Dict.toList params |> List.filter (\( _, v ) -> v /= "")
    in
    case pairs of
        [] ->
            path

        _ ->
            path ++ "?" ++ String.join "&" (List.map (\( k, v ) -> Url.percentEncode k ++ "=" ++ Url.percentEncode v) pairs)
