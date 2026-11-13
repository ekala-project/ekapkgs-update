port module Main exposing (main)

{-| The ekapkgs-update dashboard: a single-page app over the read-only JSON
API. Every page is one `GET /api/…` request; the data is re-fetched when
the URL changes, when the server reports a database change over
`/ws/live` (via the `liveUpdate` port, see `static/js/app.js`), and once a
minute as a fallback.
-}

import Api
import Browser
import Browser.Dom
import Browser.Navigation as Nav
import Ctx exposing (Ctx)
import Dict exposing (Dict)
import Html exposing (..)
import Html.Attributes exposing (..)
import Http
import Json.Decode as D
import Msg exposing (LiveState, Msg(..))
import Page.Analytics
import Page.Dashboard
import Page.Failures
import Page.Package
import Page.Packages
import Page.Session
import Page.Sessions
import Page.Watch
import Route exposing (Route)
import Task
import Time
import Url exposing (Url)


port liveStatus : (LiveState -> msg) -> Sub msg


port liveUpdate : (String -> msg) -> Sub msg


type alias Flags =
    { now : Int }


main : Program Flags Model Msg
main =
    Browser.application
        { init = init
        , view = view
        , update = update
        , subscriptions = subscriptions
        , onUrlRequest = LinkClicked
        , onUrlChange = UrlChanged
        }



-- MODEL


type PageData
    = DashboardData Page.Dashboard.Data
    | PackagesData Page.Packages.Data
    | PackageData Page.Package.Data
    | SessionsData Page.Sessions.Data
    | SessionData Page.Session.Data
    | FailuresData Page.Failures.Data
    | WatchData Page.Watch.Data
    | AnalyticsData Page.Analytics.Data


type alias Model =
    { key : Nav.Key
    , url : Url
    , route : Route
    , now : Time.Posix
    , live : LiveState
    , drafts : Dict String String

    -- Sequence number of the latest request; older responses are dropped.
    , seq : Int
    , loading : Bool
    , data : Maybe PageData
    , error : Maybe Api.Error
    }


init : Flags -> Url -> Nav.Key -> ( Model, Cmd Msg )
init flags url key =
    fetch
        { key = key
        , url = url
        , route = Route.fromUrl url
        , now = Time.millisToPosix flags.now
        , live = { state = "connecting", label = "connecting" }
        , drafts = Dict.empty
        , seq = 0
        , loading = False
        , data = Nothing
        , error = Nothing
        }


fetch : Model -> ( Model, Cmd Msg )
fetch model =
    case Route.apiUrl model.route model.url.query of
        Just url ->
            let
                seq =
                    model.seq + 1
            in
            ( { model | seq = seq, loading = True }
            , Http.get { url = url, expect = Api.expectValue (GotData seq) }
            )

        Nothing ->
            ( { model | loading = False, data = Nothing, error = Nothing }, Cmd.none )


decoderFor : Route -> D.Decoder PageData
decoderFor route =
    case route of
        Route.Dashboard ->
            D.map DashboardData Page.Dashboard.decoder

        Route.Packages ->
            D.map PackagesData Page.Packages.decoder

        Route.Package _ ->
            D.map PackageData Page.Package.decoder

        Route.Sessions ->
            D.map SessionsData Page.Sessions.decoder

        Route.Session _ ->
            D.map SessionData Page.Session.decoder

        Route.Failures ->
            D.map FailuresData Page.Failures.decoder

        Route.Watch ->
            D.map WatchData Page.Watch.decoder

        Route.Analytics ->
            D.map AnalyticsData Page.Analytics.decoder

        Route.NotFound ->
            D.fail "no data for this page"



-- UPDATE


update : Msg -> Model -> ( Model, Cmd Msg )
update msg model =
    case msg of
        LinkClicked (Browser.Internal url) ->
            if isAppPath url.path then
                ( model, Nav.pushUrl model.key (Url.toString url) )

            else
                -- /api/… and /static/… are real files, not app pages.
                ( model, Nav.load (Url.toString url) )

        LinkClicked (Browser.External href) ->
            ( model, Nav.load href )

        UrlChanged url ->
            let
                route =
                    Route.fromUrl url

                same =
                    Route.samePage route model.route

                ( next, cmd ) =
                    fetch
                        { model
                            | url = url
                            , route = route
                            , drafts = Dict.empty
                            , data =
                                if same then
                                    model.data

                                else
                                    Nothing
                            , error = Nothing
                        }
            in
            ( next
            , if same then
                cmd

              else
                Cmd.batch [ cmd, Task.perform (\_ -> NoOp) (Browser.Dom.setViewport 0 0) ]
            )

        GotData seq result ->
            if seq /= model.seq then
                ( model, Cmd.none )

            else
                case Result.andThen (decodeValue model.route) result of
                    Ok data ->
                        ( { model | loading = False, data = Just data, error = Nothing }, Cmd.none )

                    Err err ->
                        ( { model | loading = False, error = Just err }, Cmd.none )

        Tick now ->
            ( { model | now = now }, Cmd.none )

        Refresh ->
            if model.loading then
                ( model, Cmd.none )

            else
                fetch model

        LiveStatus live ->
            ( { model | live = live }, Cmd.none )

        LiveUpdate _ ->
            fetch model

        Draft key value ->
            ( { model | drafts = Dict.insert key value model.drafts }, Cmd.none )

        Submit path fields ->
            let
                ctx =
                    context model

                params =
                    List.foldl (\k acc -> Dict.insert k (Ctx.field ctx k) acc)
                        (Dict.remove "page" ctx.query)
                        fields
            in
            ( model, Nav.pushUrl model.key (Route.withQuery path params) )

        NoOp ->
            ( model, Cmd.none )


decodeValue : Route -> D.Value -> Result Api.Error PageData
decodeValue route value =
    D.decodeValue (decoderFor route) value
        |> Result.mapError (\e -> Api.Failed ("The dashboard could not read the server's response: " ++ D.errorToString e))


isAppPath : String -> Bool
isAppPath path =
    not (String.startsWith "/api/" path || String.startsWith "/static/" path || String.startsWith "/ws/" path)


context : Model -> Ctx
context model =
    { now = model.now
    , path = model.url.path
    , query = Route.parseQuery model.url.query
    , drafts = model.drafts
    }



-- SUBSCRIPTIONS


subscriptions : Model -> Sub Msg
subscriptions _ =
    Sub.batch
        [ liveStatus LiveStatus
        , liveUpdate LiveUpdate

        -- Keep relative times current.
        , Time.every 15000 Tick

        -- Fallback refresh when the live connection is down.
        , Time.every 60000 (\_ -> Refresh)
        ]



-- VIEW


view : Model -> Browser.Document Msg
view model =
    let
        page =
            viewPage model
    in
    { title = page.title ++ " · ekapkgs-update"
    , body =
        [ a [ class "skip-link", href "#main" ] [ text "Skip to content" ]
        , header [ class "site-header" ]
            [ div [ class "site-header-inner" ]
                [ a [ class "brand", href "/" ] [ text "ekapkgs-update" ]
                , nav [ class "site-nav", attribute "aria-label" "Main" ]
                    (List.map (navLink model.route)
                        [ ( "/", "Dashboard", Route.Dashboard )
                        , ( "/packages", "Packages", Route.Packages )
                        , ( "/sessions", "Sessions", Route.Sessions )
                        , ( "/failures", "Failures", Route.Failures )
                        , ( "/watch", "Watch", Route.Watch )
                        , ( "/analytics", "Analytics", Route.Analytics )
                        ]
                    )
                , span
                    [ id "live-status"
                    , class "live-status"
                    , attribute "data-state" model.live.state
                    , attribute "role" "status"
                    , title ("Live updates: " ++ model.live.label)
                    ]
                    [ span [ class "dot", attribute "aria-hidden" "true" ] []
                    , span [ class "live-label" ] [ text model.live.label ]
                    ]
                ]
            ]
        , main_ [ id "main", class "page", attribute "aria-busy" (boolAttr model.loading) ] page.body
        , footer [ class "site-footer" ]
            [ p [] [ text "Read-only view of the ekapkgs-update database. Times are relative; hover one for the exact UTC time." ] ]
        ]
    }


boolAttr : Bool -> String
boolAttr b =
    if b then
        "true"

    else
        "false"


navLink : Route -> ( String, String, Route ) -> Html msg
navLink current ( path, label, route ) =
    let
        active =
            case ( current, route ) of
                ( Route.Package _, Route.Packages ) ->
                    True

                ( Route.Session _, Route.Sessions ) ->
                    True

                _ ->
                    current == route
    in
    a
        (href path
            :: (if active then
                    [ attribute "aria-current" "page" ]

                else
                    []
               )
        )
        [ text label ]


viewPage : Model -> Ctx.Page
viewPage model =
    let
        ctx =
            context model
    in
    case ( model.route, model.data, model.error ) of
        ( Route.NotFound, _, _ ) ->
            errorPage 404 "Not found" "There is no page at this address."

        ( _, _, Just (Api.NotFound message) ) ->
            errorPage 404 "Not found" message

        ( _, Nothing, Just (Api.Failed message) ) ->
            errorPage 500 "Something went wrong" message

        ( _, Nothing, Nothing ) ->
            { title = "Loading", body = [ p [ class "loading muted" ] [ text "Loading…" ] ] }

        ( _, Just data, err ) ->
            let
                page =
                    viewData ctx data
            in
            case err of
                Just (Api.Failed message) ->
                    { page
                        | body =
                            div [ class "notice tone-serious", attribute "role" "alert" ]
                                [ text ("Could not refresh this page: " ++ message ++ " Showing the last data received.") ]
                                :: page.body
                    }

                _ ->
                    page


viewData : Ctx -> PageData -> Ctx.Page
viewData ctx data =
    case data of
        DashboardData d ->
            Page.Dashboard.view ctx d

        PackagesData d ->
            Page.Packages.view ctx d

        PackageData d ->
            Page.Package.view ctx d

        SessionsData d ->
            Page.Sessions.view ctx d

        SessionData d ->
            Page.Session.view ctx d

        FailuresData d ->
            Page.Failures.view ctx d

        WatchData d ->
            Page.Watch.view ctx d

        AnalyticsData d ->
            Page.Analytics.view ctx d


errorPage : Int -> String -> String -> Ctx.Page
errorPage code heading message =
    { title = heading
    , body =
        [ section [ class "error-page" ]
            [ p [ class "error-code" ] [ text (String.fromInt code) ]
            , h1 [] [ text heading ]
            , p [] [ text message ]
            , p [] [ a [ class "button", href "/" ] [ text "Back to the dashboard" ] ]
            ]
        ]
    }
