module RouteTest exposing (suite)

import Dict
import Expect
import Route exposing (Route(..))
import Table exposing (cases)
import Test exposing (Test, describe, test)
import Url


routeOf : String -> Maybe Route
routeOf path =
    Url.fromString ("http://localhost:8080" ++ path) |> Maybe.map Route.fromUrl


suite : Test
suite =
    describe "Route"
        [ cases "fromUrl" routeOf
            [ ( "/", Just Dashboard )
            , ( "", Just Dashboard )
            , ( "/?repo=corepkgs", Just Dashboard )
            , ( "/packages", Just Packages )
            , ( "/packages/", Just Packages )
            , ( "/packages?q=foo&page=2", Just Packages )
            , ( "/packages/hello", Just (Package "hello") )
            , ( "/packages/python3Packages.requests", Just (Package "python3Packages.requests") )
            , ( "/packages/foo%20bar", Just (Package "foo bar") )
            , ( "/packages/a%2Fb", Just (Package "a/b") )
            , ( "/packages/a%25b", Just (Package "a%b") )
            , ( "/packages/%ZZ", Just (Package "%ZZ") )
            , ( "/packages/hello?repo=default#phases", Just (Package "hello") )
            , ( "/sessions", Just Sessions )
            , ( "/sessions/1b9d6bcd-bbfd-4b2d-9b5d-ab8dfbbd4bed", Just (Session "1b9d6bcd-bbfd-4b2d-9b5d-ab8dfbbd4bed") )
            , ( "/failures", Just Failures )
            , ( "/failures?phase=build", Just Failures )
            , ( "/watch", Just Watch )
            , ( "/analytics", Just Analytics )
            , ( "/nope", Just NotFound )
            , ( "/packages/a/b", Just NotFound )
            , ( "/sessions/a/b", Just NotFound )
            , ( "/watch/extra", Just NotFound )
            , ( "/api/dashboard", Just NotFound )
            , ( "/static/css/styles.css", Just NotFound )
            , ( "/Packages", Just NotFound )
            ]
        , cases "apiUrl (route, query)" (\( route, query ) -> Route.apiUrl route query)
            [ ( ( Dashboard, Nothing ), Just "/api/dashboard" )
            , ( ( Dashboard, Just "repo=corepkgs" ), Just "/api/dashboard?repo=corepkgs" )
            , ( ( Packages, Just "" ), Just "/api/packages" )
            , ( ( Packages, Just "q=foo+bar&page=2&status=failed" ), Just "/api/packages?q=foo+bar&page=2&status=failed" )
            , ( ( Package "hello", Nothing ), Just "/api/packages/hello" )
            , ( ( Package "foo bar", Just "repo=default" ), Just "/api/packages/foo%20bar?repo=default" )
            , ( ( Package "a/b", Nothing ), Just "/api/packages/a%2Fb" )
            , ( ( Sessions, Just "page=3" ), Just "/api/sessions?page=3" )
            , ( ( Session "abc-123", Nothing ), Just "/api/sessions/abc-123" )
            , ( ( Failures, Just "phase=build" ), Just "/api/failures?phase=build" )
            , ( ( Watch, Nothing ), Just "/api/watch" )
            , ( ( Analytics, Just "days=30" ), Just "/api/analytics?days=30" )
            , ( ( NotFound, Nothing ), Nothing )
            , ( ( NotFound, Just "q=x" ), Nothing )
            ]
        , test "fromUrl and apiUrl agree on encoded attribute paths" <|
            \_ ->
                Expect.equal (Just (Just "/api/packages/a%2Fb%20c"))
                    (routeOf "/packages/a%2Fb%20c" |> Maybe.map (\r -> Route.apiUrl r Nothing))
        , cases "samePage" (\( a, b ) -> Route.samePage a b)
            [ ( ( Dashboard, Dashboard ), True )
            , ( ( Dashboard, Packages ), False )
            , ( ( Package "a", Package "a" ), True )
            , ( ( Package "a", Package "b" ), False )
            , ( ( Session "x", Session "x" ), True )
            , ( ( Session "x", Session "y" ), False )
            , ( ( Package "x", Session "x" ), False )
            , ( ( NotFound, NotFound ), True )
            ]
        , cases "parseQuery" (Route.parseQuery >> Dict.toList)
            [ ( Nothing, [] )
            , ( Just "", [] )
            , ( Just "a=1&b=x+y", [ ( "a", "1" ), ( "b", "x y" ) ] )
            , ( Just "q=foo%20bar%2Bbaz", [ ( "q", "foo bar+baz" ) ] )
            , ( Just "q=caf%C3%A9", [ ( "q", "café" ) ] )
            , ( Just "k%20ey=v", [ ( "k ey", "v" ) ] )
            , ( Just "flag", [ ( "flag", "" ) ] )
            , ( Just "flag=", [ ( "flag", "" ) ] )
            , ( Just "a=1=2", [ ( "a", "1=2" ) ] )
            , ( Just "a=1&&b=2&", [ ( "a", "1" ), ( "b", "2" ) ] )
            , ( Just "a=1&a=2", [ ( "a", "2" ) ] )
            , ( Just "q=50%", [ ( "q", "50%" ) ] )
            , ( Just "q=a+100%", [ ( "q", "a 100%" ) ] )
            ]
        , cases "withQuery (path, pairs)" (\( path, pairs ) -> Route.withQuery path (Dict.fromList pairs))
            [ ( ( "/packages", [] ), "/packages" )
            , ( ( "/packages", [ ( "status", "" ) ] ), "/packages" )
            , ( ( "/packages", [ ( "q", "a b" ), ( "page", "2" ), ( "status", "" ) ] ), "/packages?page=2&q=a%20b" )
            , ( ( "/failures", [ ( "phase", "build" ), ( "q", "x+y&z=1" ) ] ), "/failures?phase=build&q=x%2By%26z%3D1" )
            , ( ( "/", [ ( "a key", "café" ) ] ), "/?a%20key=caf%C3%A9" )
            ]
        , cases "parseQuery inverts withQuery"
            (\pairs ->
                let
                    params =
                        Dict.fromList pairs
                in
                case String.split "?" (Route.withQuery "/p" params) of
                    [ _, query ] ->
                        Route.parseQuery (Just query) == Dict.filter (\_ v -> v /= "") params

                    _ ->
                        Dict.isEmpty (Dict.filter (\_ v -> v /= "") params)
            )
            [ ( [], True )
            , ( [ ( "q", "a b+c&d=e" ), ( "page", "2" ) ], True )
            , ( [ ( "repo", "default" ), ( "status", "" ) ], True )
            , ( [ ( "q", "100% ?#/" ) ], True )
            ]
        ]
