use std::{collections::HashMap, fmt, sync::Arc};
use uuid::Uuid;

use oauth2::{self, TokenResponse, CsrfToken};
use oauth2::basic::BasicClient;
use iron::{self, AroundMiddleware};
use iron::prelude::*;
use iron::{Handler, BeforeMiddleware, AfterMiddleware, status, modifiers::RedirectRaw, headers};
use cookie::{Cookie, CookieBuilder};
use typemap::Key;
use handlebars_iron::{Template, HandlebarsEngine, DirectorySource};
#[cfg(feature = "watch")]
use handlebars_iron::Watchable;
use router::{Router, NoRoute};
use staticfile::Static;
use mount::Mount;
use params::{Params, Value};
use serenity::model::id;
use serenity::model::guild;
use serde_derive::{Deserialize, Serialize};
use serde_json;
use log::debug;
use futures_lite::future;

use model::*;
use data::*;
use bottle;

#[derive(Debug)]
struct InternalError(String);
#[derive(Debug)]
struct ParamError;
#[derive(Debug)]
struct AuthError;

#[derive(Clone, Deserialize, Serialize)]
struct SessionData {
    id: Uuid,
    redirect: Option<String>,
    csrf: Option<String>
}

struct DSessionData;
impl Key for DSessionData {
    type Value = SessionData;
}

impl SessionData {
    fn new() -> Self {
        SessionData { id: Uuid::new_v4(), redirect: None, csrf: None}
    }

    fn to_cookie(&self, cfg: &Config) -> Cookie {
        Cookie::build("session", serde_json::to_string(self).unwrap())
            .domain(cfg.host_domain).path(cfg.host_path).secure(true).finish()
    }

    
}

struct SessionStorage {}

impl AroundMiddleware for SessionStorage {
    fn around(self, handler: Box<dyn Handler>) -> Box<dyn Handler> {
        Box::new(move |req: &mut Request| -> IronResult<Response> {
            let ses = match req.headers.iter()
                .filter(|x| x.is::<headers::Cookie>())
                .filter_map(|x| x.value::<headers::Cookie>())
                .next().and_then(|c| c.0.iter().filter_map(|s| Cookie::parse(s).ok())
                    .find(|c| c.name()=="session")) {
                Some(cookie) =>
                    serde_json::from_str(cookie.value())
                        .map_err(|e| IronError::new(ParamError, status::BadRequest))?,
                _ => SessionData::new()
            };

            req.extensions.insert::<DSessionData>(ses);
            let mut resp = handler.handle(req)?;

            let data = req.extensions.get::<DSessionData>().unwrap();
            resp.headers.set(headers::SetCookie(vec![data.to_cookie(req.get_cfg()).to_string()]));
            Ok(resp)
        })
    }
}

trait GetSession {
    fn session(&mut self) -> &mut SessionData;
}

impl<'a,'b> GetSession for Request<'a,'b> {
    fn session(&mut self) -> &mut SessionData {
        self.extensions.get_mut::<DSessionData>().unwrap()
    }
}

impl fmt::Display for InternalError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let InternalError(desc) = self;
        write!(f, "An internal error occured: {}", desc)
    }
}

impl iron::Error for InternalError {}

impl InternalError {
    fn with<T, F: FnMut() -> Res<T>>(mut f: F) -> IronResult<T> {
        f().map_err(|err| {
            IronError::new(InternalError(err.to_string()), status::InternalServerError)
        })
    }
}

impl fmt::Display for ParamError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Error finding/parsing a parameter")
    }
}

impl iron::Error for ParamError {}

impl fmt::Display for AuthError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Error authorizing")
    }
}

impl iron::Error for AuthError {}

struct PrerequisiteMiddleware {pool: ConnPool, oauth: BasicClient, cfg: Config}

impl BeforeMiddleware for PrerequisiteMiddleware {
    fn before(&self, req: &mut Request) -> IronResult<()> {
        req.extensions.insert::<DConn>(self.pool.clone());
        req.extensions.insert::<DOauth2>(self.oauth.clone());
        req.extensions.insert::<DConfig>(self.cfg.clone());

        Ok(())
    }
}

impl<'a, 'b> GetConnection for Request<'a, 'b> {
    fn get_pool(&self) -> ConnPool {
        self.extensions.get::<DConn>().unwrap().get_pool()
    }
}

impl<'a, 'b> GetConfig for Request<'a, 'b> {
    fn get_cfg(&self) -> &Config {
        self.extensions.get::<DConfig>().unwrap()
    }
}

struct StatusMiddleware;
impl AfterMiddleware for StatusMiddleware {
    fn catch(&self, _req: &mut Request, err: IronError) -> IronResult<Response> {
        if err.error.is::<NoRoute>() || err.error.is::<ParamError>() {
            Ok(Response::with((status::NotFound, Template::new("notfound", &false))))
        } else {
            Err(err)
        }
    }
}

#[derive(Deserialize, Serialize)]
struct BottlePage {
    contents: String, time_pushed: String, image: Option<String>, guild: Option<String>
}

#[derive(Deserialize, Serialize)]
struct GuildContribution {guild: String, gid: i64, xp: i64}
#[derive(Deserialize, Serialize)]
struct UserPage {
    tag: String, admin: bool, pfp: String, xp: i32, ranked: i64, num_bottles: i64, contributions: Vec<GuildContribution>, recent_bottles: Vec<BottlePage>
}

#[derive(Deserialize, Serialize)]
struct UserContribution {user: String, uid: i64, xp: i64}
#[derive(Deserialize, Serialize)]
struct GuildPage {
    name: String, pfp: String, invite: Option<String>, xp: i64, ranked: Option<i64>, num_bottles: i64, contributions: Vec<UserContribution>
}

fn get_user_data(uid: UserId, conn: &Conn, cfg: &Config) -> Res<UserPage> {
    debug!("Getting user page data for {}", uid);

    let udata = User::get(uid, conn);
    let user = id::UserId(udata.id as u64).to_user()?;

    let data = UserPage {
        tag: user.tag(), admin: udata.admin,
        pfp: user.avatar_url().unwrap_or_else(|| anonymous_url(cfg)),
        xp: udata.xp,
        ranked: udata.get_ranking(conn)?,
        num_bottles: udata.get_num_bottles(conn)?,
        contributions: udata.get_contributions(5, conn)?.into_iter().map(|c| {
            GuildContribution {guild: get_guild_name(c.guild), gid: c.guild, xp: c.xp as i64}
        }).collect(),
        recent_bottles: udata.get_last_bottles(10, conn)?.into_iter().map(|bottle| {
            BottlePage {
                contents: bottle.contents,
                time_pushed: bottle.time_pushed.format(&"%m/%d/%y - %H:%M").to_string(),
                image: bottle.image,
                guild: bottle.guild.map(get_guild_name)
            }
        }).collect()
    };

    Ok(data)
}

fn user(req: &mut Request) -> IronResult<Response> {
    let udata = req.extensions.get::<Router>().unwrap()
        .find("user").and_then(|x| x.parse().ok()).and_then(|uid| {
        get_user_data(uid, &req.get_conn(), &req.get_cfg()).ok()
    });

    match udata {
        Some(udata) => Ok(Response::with((status::Ok, Template::new("user", &udata)))),
        None => Err(IronError::new(ParamError, status::NotFound))
    }
}

fn get_guild_data(gid: GuildId, conn: &Conn, cfg: &Config) -> Res<GuildPage> {
    debug!("Getting guild page data for {}", gid);

    let gdata = Guild::get(gid, conn);
    let guild: guild::PartialGuild = id::GuildId(gid as u64).to_partial_guild()?;

    let data = GuildPage {
        name: guild.name.clone(), invite: gdata.invite.clone(),
        pfp: guild.icon_url().unwrap_or_else(|| anonymous_url(cfg)).clone(),
        xp: gdata.get_xp(conn)?,
        ranked: gdata.get_ranking(conn).ok(),
        num_bottles: gdata.get_num_bottles(conn)?,
        contributions: gdata.get_contributions(15, conn)?.into_iter().map(|c| {
            UserContribution {user: get_user_name(c.user), uid: c.user, xp: c.xp as i64}
        }).collect()
    };

    Ok(data)
}

fn guild(req: &mut Request) -> IronResult<Response> {
    let gdata = req.extensions.get::<Router>().unwrap()
        .find("guild").and_then(|x| x.parse().ok()).and_then(|gid| {
        get_guild_data(gid, &req.get_conn(), &req.get_cfg()).ok()
    });

    match gdata {
        Some(gdata) => Ok(Response::with((status::Ok, Template::new("guild", &gdata)))),
        None => Err(IronError::new(ParamError, status::NotFound))
    }
}

#[derive(Clone, Deserialize, Serialize, Debug)]
struct DUserData {
    id: String,
    username: String,
    discriminator: String
}

const GETUSER: &str = "https://discordapp.com/api/users/@me";
impl DUserData {
    fn get(access_token: &str) -> Res<Self> {
        let fut = reqwest::Client::new().get(GETUSER)
            .header("Authorization", format!("Bearer {}", access_token)).send();
        Ok(future::block_on(future::block_on(fut)?.json::<DUserData>())?)
    }
}

fn get_user(ses: &SessionData, conn: &Conn) -> Option<User> {
    User::from_session(ses.id, conn).ok()
}

fn set_tok(ses: &SessionData, tok: oauth2::basic::BasicTokenResponse, conn: &Conn) -> Res<()> {
    let uid = DUserData::get(tok.access_token().secret())?.id.parse()?;
    let mut u = User::get(uid, conn);

    u.session = Some(ses.id);
    u.update(conn)?;

    Ok(())
}

fn report(req: &mut Request) -> IronResult<Response> {
    let bid = req.extensions.get::<Router>().unwrap()
        .find("bottle").and_then(|x| x.parse().ok())
        .ok_or_else(|| IronError::new(ParamError, status::BadRequest))?;

    let conn = &req.get_conn();
    let ses = req.session();

    if let Ok (bottle) = Bottle::get(bid, conn) {
        match get_user(ses, conn) {
            Some(mut x) => {
                let data = InternalError::with(|| {
                    let banned = x.get_banned(conn)?;
                    let alreadyexists = Report::exists(bid, conn)?;

                    if (x.admin || !banned) && !alreadyexists {
                        let received_bottle = bottle::report_bottle(&bottle, x.id, conn, &req.get_cfg())?;
                        Report { user: x.id, bottle: bid, received_bottle: Some(received_bottle) }.make(conn)?;

                        x.xp += REPORTXP;
                        x.update(conn)?;
                    }

                    let mut data = HashMap::new();
                    data.insert("banned", banned);
                    data.insert("alreadyexists", alreadyexists);

                    Ok(data)
                })?;

                Ok(Response::with((status::Ok, Template::new("reportmade", data))))
            },
            None => {
                let (url,tok) = req.extensions.get::<DOauth2>().unwrap().clone()
                    .authorize_url(CsrfToken::new_random)
                    .add_scope(oauth2::Scope::new("identify".to_string()))
                    .url();

                ses.csrf = Some(tok.secret().to_string());
                ses.redirect = Some(report_url(bid, &req.get_cfg()));

                Ok(Response::with((status::TemporaryRedirect, RedirectRaw(url.to_string()))))
            }
        }
    } else {
        Err(IronError::new(ParamError, status::NotFound))
    }
}

fn redirect(req: &mut Request) -> IronResult<Response> {
    let params = req.get_ref::<Params>().unwrap().clone();

    let session = req.session();
    match params.find(&["state"]) {
        Some(Value::String(state)) if Some(state) == session.csrf.as_ref() => {
            if let Some(Value::String(code)) = params.find(&["code"]) {
                let oauth = req.extensions.get::<DOauth2>().unwrap().clone();

                if let Ok(tok) = oauth
                    .exchange_code(oauth2::AuthorizationCode::new(code.to_string()))
                    .request(oauth2::reqwest::http_client) {

                    let conn = &req.get_conn();
                    set_tok(req.session(), tok, conn).unwrap();

                    return match session.redirect {
                        Some(ref redirect) => Ok(Response::with((status::TemporaryRedirect, RedirectRaw(redirect.clone())))),
                        _ => Ok(Response::with(status::Ok))
                    };
                }
            }
        },
        _ => ()
    }

    Err(IronError::new(AuthError, status::BadRequest))
}

#[derive(Deserialize, Serialize)]
struct HomePage {
    bottle_count: i64,
    user_count: i64,
    guild_count: i64,

    guild_leaderboard: Vec<GuildContribution>,
    user_leaderboard: Vec<UserContribution>
}

fn home(req: &mut Request) -> IronResult<Response> {
    let conn: &Conn = &req.get_conn();

    let data = InternalError::with(|| {
        Ok(HomePage {
            bottle_count: get_bottle_count(conn).map_err(Box::new)?,
            user_count: get_user_count(conn)?,
            guild_count: get_guild_count(conn)?,

            guild_leaderboard: Guild::get_top(10, conn)?
                .into_iter().map(|x| GuildContribution {gid: x.id, guild: get_guild_name(x.id), xp: x.get_xp(conn).unwrap_or(0)}).collect(),
            user_leaderboard: User::get_top(10, conn)?
                .into_iter().map(|x| UserContribution {uid: x.id, user: get_user_name(x.id), xp: x.xp as i64}).collect(),
        })
    })?;

    let resp = Response::with((status::Ok, Template::new("home", &data)));
    Ok(resp)
}

#[cfg(feature = "watch")]
fn watch_serv(hbse: &Arc<HandlebarsEngine>) {
    hbse.watch("./res/");
}

#[cfg(not(feature = "watch"))]
fn watch_serv(_: &Arc<HandlebarsEngine>) {
    ()
}

pub fn start_serv (db: ConnPool, cfg: Config) {
    let reqcfg = cfg.clone();
    let oauthcfg = BasicClient::new(
        oauth2::ClientId::new(cfg.client_id),
        Some(oauth2::ClientSecret::new(cfg.client_secret)),
        oauth2::AuthUrl::new("https://discordapp.com/api/oauth2/authorize".to_string()).unwrap(),
        Some(oauth2::TokenUrl::new("https://discordapp.com/api/oauth2/token".to_string()).unwrap())
    )
        .set_redirect_url(format!("{}/oauth", cfg.host_url));

    let mut router = Router::new();
    router.get("/", home, "home");
    router.get("/u/:user", user, "user");
    router.get("/g/:guild", guild, "guild");
    router.get("/report/:bottle", report, "report");
    router.get("/oauth", redirect, "redirect");

    let mut chain = Chain::new(router);

    let mut hbse = HandlebarsEngine::new();
    hbse.add(Box::new(DirectorySource::new("./res/", ".html")));
    hbse.reload().unwrap();

    let hbse_r = Arc::new(hbse);
    watch_serv(&hbse_r);

    chain.link_around(SessionStorage {});
    chain.link_before(PrerequisiteMiddleware {pool: db, oauth: oauthcfg, cfg: reqcfg});
    chain.link_after(StatusMiddleware);
    chain.link_after(hbse_r);

    let mut mount = Mount::new();

    mount.mount("/", chain);
    mount.mount("/style", Static::new("./res/style"));
    mount.mount("/img", Static::new("./res/img"));

    let iron = Iron::new(mount);
    let _ = iron.http("0.0.0.0:8080", ).unwrap();
}