//! Expansion of an architecture style into import rules (`arch:<style>:<id>`).

use super::{ArchRule, ArchRuleKind, ArchTarget, ArchitectureStyle, ModuleIsolation};

const INFRA: &[&str] = &["infrastructure", "infraestructure", "infra", "adapters", "persistence"];
const DELIVERY: &[&str] =
    &["controller", "controllers", "presentation", "api", "http", "web", "graphql"];
/// Directories that every module may import freely (shared kernels) and that are not modules.
const SHARED_MODULES: &[&str] =
    &["shared", "common", "core", "config", "utils", "lib", "libs", "kernel", "modules"];

const DOMAIN_PACKAGES: &[&str] = &[
    "@nestjs/*",
    "typeorm",
    "@prisma/*",
    "mongoose",
    "sequelize",
    "express",
    "fastify",
    "class-validator",
    "class-transformer",
    "axios",
    "pg",
    "mysql2",
    "redis",
    "ioredis",
];
const APPLICATION_PACKAGES: &[&str] =
    &["typeorm", "@prisma/*", "mongoose", "sequelize", "express", "fastify", "axios", "pg"];

const FSD_LAYERS: &[&str] =
    &["app", "processes", "pages", "widgets", "features", "entities", "shared"];

const DOMAIN_NO_APPLICATION: &str = "domain/ is the core and must not import application/. Move the shared type into domain/, or let the use case call the domain instead of the reverse.";
const DOMAIN_NO_INFRA: &str = "domain/ must not import infrastructure code. Declare a port (interface) in domain/, implement it in infrastructure/, and inject it by token (I<X>Repository -> Pg<X>Repository).";
const DOMAIN_NO_DELIVERY: &str = "domain/ must not import controllers or other delivery code. Delivery calls the use cases, never the other way around.";
const APPLICATION_NO_INFRA: &str = "application/ depends on ports declared in domain/, not on adapters. Inject the port by token and bind the adapter in the *.module.ts.";
const APPLICATION_NO_DELIVERY: &str = "application/ must not import controllers or delivery DTOs. Define the input as a command inside application/ and map it in the controller.";
const INFRA_NO_DELIVERY: &str = "infrastructure/ implements ports and must not import controllers. Move the shared type to domain/ or application/.";
const DOMAIN_NO_FRAMEWORK: &str = "domain/ must stay framework-free. Keep plain TypeScript here; put decorators, ORM entities and validation in infrastructure/ or in the controller DTOs and map them to domain objects.";
const APPLICATION_NO_ORM: &str = "application/ must not import ORM or HTTP packages. Depend on a port declared in domain/ and implement it in infrastructure/; @nestjs/common (@Injectable, @Inject) stays allowed.";
const APPLICATION_NO_ORM_STRICT: &str = "application/ must not import ORM, HTTP or @nestjs/* packages (strict_framework). Depend on a port declared in domain/ and implement it in infrastructure/.";
const MODULE_BOUNDARIES: &str = "A module may use another module only through its domain/, application/, *.module.ts or index.ts, never its infrastructure or delivery folders. Expose what you need there or call the other module's use case.";
const CLEAN_REASON: &str = "Dependencies point inward (entities <- use cases <- interface adapters <- frameworks). Define an interface in the inner ring and implement it in the outer one.";
const DATA_NO_UPWARD: &str = "Repositories, DAOs and models sit at the bottom and must not import services, controllers or routes. Pass what they need as arguments or return values.";
const SERVICES_NO_CONTROLLERS: &str = "Services must not import controllers or routes. Controllers call services; move shared types into the service layer.";
const SLICES_ISOLATED: &str = "Slices of one layer must not import each other. Move the shared code down to entities/shared or compose the slices in a higher layer (pages/widgets).";
const PUBLIC_API_REASON: &str = "Modules talk to each other only through their public API (index.*, *.module.*, public/, api/). Export what the other module needs there instead of importing its internals.";
const HANDLER_ISOLATION: &str = "Handlers, consumers and subscribers must not reach into another context's internals. Consume its published events (events/, contracts/) or publish a command through the event bus.";

/// Expands the style of `target` into rules whose globs are anchored under its source directory.
pub fn expand(target: &ArchTarget<'_>) -> Vec<ArchRule> {
    let style = target.setting.style;
    let ctx = Ctx {
        style,
        root: target.source_dir(),
        suffix: target.subrepo.map(|name| format!("@{name}")).unwrap_or_default(),
        modules: target.setting.modules.clone(),
        strict: target.setting.strict_framework.unwrap_or(false),
    };
    match style {
        ArchitectureStyle::None => Vec::new(),
        ArchitectureStyle::Hexagonal => hexagonal(&ctx),
        ArchitectureStyle::Clean => clean(&ctx),
        ArchitectureStyle::Layered => layered(&ctx),
        ArchitectureStyle::FeatureSliced => feature_sliced(&ctx),
        ArchitectureStyle::ModularMonolith => modular_monolith(&ctx),
        ArchitectureStyle::EventDriven => event_driven(&ctx),
    }
}

struct Ctx {
    style: ArchitectureStyle,
    root: String,
    suffix: String,
    modules: Vec<String>,
    strict: bool,
}

impl Ctx {
    fn under(&self, rest: &str) -> String {
        if self.root.is_empty() { rest.to_owned() } else { format!("{}/{rest}", self.root) }
    }

    /// `<root>/**/<name>/**` for each folder name.
    fn folders(&self, names: &[&str]) -> Vec<String> {
        names.iter().map(|name| self.under(&format!("**/{name}/**"))).collect()
    }

    fn name(&self, id: &str) -> String {
        format!("arch:{}:{id}{}", self.style, self.suffix)
    }

    fn layer(&self, id: &str, reason: &str, from: &[String], forbid: &[String]) -> ArchRule {
        ArchRule {
            name: self.name(id),
            reason: reason.to_owned(),
            kind: ArchRuleKind::Layer { from: from.to_vec(), forbid: forbid.to_vec() },
        }
    }

    fn packages(&self, id: &str, reason: &str, from: &[String], packages: &[&str]) -> ArchRule {
        ArchRule {
            name: self.name(id),
            reason: reason.to_owned(),
            kind: ArchRuleKind::Packages { from: from.to_vec(), packages: strings(packages) },
        }
    }

    fn isolation(&self, id: &str, reason: &str, iso: ModuleIsolation) -> ArchRule {
        ArchRule {
            name: self.name(id),
            reason: reason.to_owned(),
            kind: ArchRuleKind::Modules(iso),
        }
    }

    /// Modules are the directories under `<root>/modules` or, without that folder, under `<root>`.
    fn module_roots(&self) -> Vec<String> {
        let root = if self.root.is_empty() { ".".to_owned() } else { self.root.clone() };
        vec![self.under("modules"), root]
    }

    fn shared_modules(&self, public: Vec<String>, from: Vec<String>) -> ModuleIsolation {
        ModuleIsolation {
            roots: self.module_roots(),
            only: self.modules.clone(),
            exempt: strings(SHARED_MODULES),
            public,
            from,
        }
    }
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_owned()).collect()
}

fn hexagonal(ctx: &Ctx) -> Vec<ArchRule> {
    let domain = ctx.folders(&["domain"]);
    let application = ctx.folders(&["application"]);
    let infra = ctx.folders(INFRA);
    let delivery = ctx.folders(DELIVERY);
    let strict_nest = ctx.strict.then_some("@nestjs/*");
    let app_packages: Vec<&str> = APPLICATION_PACKAGES.iter().copied().chain(strict_nest).collect();
    let app_reason = if ctx.strict { APPLICATION_NO_ORM_STRICT } else { APPLICATION_NO_ORM };
    let public = strings(&["domain/**", "application/**", "*.module.*", "index.*"]);
    vec![
        ctx.layer("domain-no-application", DOMAIN_NO_APPLICATION, &domain, &application),
        ctx.layer("domain-no-infrastructure", DOMAIN_NO_INFRA, &domain, &infra),
        ctx.layer("domain-no-delivery", DOMAIN_NO_DELIVERY, &domain, &delivery),
        ctx.layer("application-no-infrastructure", APPLICATION_NO_INFRA, &application, &infra),
        ctx.layer("application-no-delivery", APPLICATION_NO_DELIVERY, &application, &delivery),
        ctx.layer("infrastructure-no-delivery", INFRA_NO_DELIVERY, &infra, &delivery),
        ctx.packages("domain-no-framework", DOMAIN_NO_FRAMEWORK, &domain, DOMAIN_PACKAGES),
        ctx.packages("application-no-orm-http", app_reason, &application, &app_packages),
        ctx.isolation(
            "module-boundaries",
            MODULE_BOUNDARIES,
            ctx.shared_modules(public, Vec::new()),
        ),
    ]
}

/// One rule per ring: a ring never imports a ring listed after it.
fn inward_rules(ctx: &Ctx, rings: &[(&str, &[&str])], reason: &str) -> Vec<ArchRule> {
    let globs: Vec<Vec<String>> = rings.iter().map(|(_, names)| ctx.folders(names)).collect();
    (0..rings.len().saturating_sub(1))
        .map(|i| {
            let outer: Vec<String> = globs[i + 1..].concat();
            ctx.layer(&format!("{}-inward-only", rings[i].0), reason, &globs[i], &outer)
        })
        .collect()
}

fn clean(ctx: &Ctx) -> Vec<ArchRule> {
    let rings: [(&str, &[&str]); 4] = [
        ("entities", &["entities", "domain"]),
        ("use-cases", &["use-cases", "usecases", "use_cases", "application"]),
        ("adapters", &["adapters", "interface-adapters", "controllers", "presenters", "gateways"]),
        ("frameworks", &["frameworks", "infrastructure", "drivers", "main"]),
    ];
    inward_rules(ctx, &rings, CLEAN_REASON)
}

fn layered(ctx: &Ctx) -> Vec<ArchRule> {
    let data = ctx.folders(&["repository", "repositories", "dao", "models"]);
    let services = ctx.folders(&["service", "services"]);
    let delivery = ctx.folders(&["controller", "controllers", "routes"]);
    let upper: Vec<String> = [services.clone(), delivery.clone()].concat();
    vec![
        ctx.layer("data-no-upward", DATA_NO_UPWARD, &data, &upper),
        ctx.layer("services-no-controllers", SERVICES_NO_CONTROLLERS, &services, &delivery),
    ]
}

fn feature_sliced(ctx: &Ctx) -> Vec<ArchRule> {
    let layer_glob = |layer: &str| ctx.under(&format!("{layer}/**"));
    let order = FSD_LAYERS.join(" > ");
    let mut rules = Vec::new();
    for (i, layer) in FSD_LAYERS.iter().enumerate().skip(1) {
        let higher: Vec<String> = FSD_LAYERS[..i].iter().map(|l| layer_glob(l)).collect();
        let reason = format!(
            "A layer imports only from layers below it ({order}). Move the shared code down a layer or compose it in a higher one."
        );
        let id = format!("{layer}-imports-downward");
        rules.push(ctx.layer(&id, &reason, &[layer_glob(layer)], &higher));
    }
    for layer in &FSD_LAYERS[1..FSD_LAYERS.len() - 1] {
        let iso = ModuleIsolation { roots: vec![ctx.under(layer)], ..ModuleIsolation::default() };
        rules.push(ctx.isolation(&format!("{layer}-slices-isolated"), SLICES_ISOLATED, iso));
    }
    rules
}

fn public_api() -> Vec<String> {
    strings(&["index.*", "*.module.*", "public/**", "api/**"])
}

fn modular_monolith(ctx: &Ctx) -> Vec<ArchRule> {
    let iso = ctx.shared_modules(public_api(), Vec::new());
    vec![ctx.isolation("module-public-api", PUBLIC_API_REASON, iso)]
}

fn event_driven(ctx: &Ctx) -> Vec<ArchRule> {
    let mut public = public_api();
    public.extend(strings(&["events/**", "contracts/**", "**/*.event.*", "**/*.events.*"]));
    let handlers = ctx.folders(&["handlers", "consumers", "subscribers"]);
    let iso = ctx.shared_modules(public, handlers);
    vec![ctx.isolation("handler-context-isolation", HANDLER_ISOLATION, iso)]
}

#[cfg(test)]
mod tests;
