// policies.cedar — who may do what, and through which gateway. Nothing here says how narrowly the
// broker executes it: timeouts, hosts, methods, call budgets, and credential binding live in
// broker.yaml's constraint sets. Editing policy may broaden who can act; it cannot reach a new host
// or bind authority that was not already constrained.
//
// chmod 0600, owner-owned, single-link. Cedar validates this in strict mode at startup against a
// schema generated from broker.yaml's declared world, so a typo in a principal, provider, or
// capability refuses startup instead of becoming a rule that silently never matches.
//
// The @id annotations are what audit records carry as `policy_ids`. Without them Cedar names
// policies positionally, which is unstable when a statement is inserted.

// Session gate: may this person drive this agent at all, and through which gateway? Checked before
// `capabilitiesFor` answers, so an unauthorized sender is refused before a model call costs anything.
@id("artist-may-prompt-chat-image-studio")
permit(principal == Dekopon::Principal::"artist",
       action == Dekopon::Action::"agent.prompt",
       resource == Dekopon::Agent::"chat-image-studio")
when { context has via && context.via == "dekopond-gateway" };

// Capability surface: driving the agent implies neither of these actions, and holding them implies no
// session. `context.via` keeps attested and direct authority disjoint; `context.agent` pins the
// surface to this one agent. Each statement is independently revocable — deleting the second one
// below leaves generation working and stops remixing the sender's own photographs.
@id("chat-image-studio-generate-for-artist")
permit(principal == Dekopon::Principal::"artist",
       action == Dekopon::Action::"gpt-image.generate",
       resource == Dekopon::Provider::"gpt-image")
when { context has agent && context.agent == "chat-image-studio"
    && context has via && context.via == "dekopond-gateway" };

@id("chat-image-studio-edit-for-artist")
permit(principal == Dekopon::Principal::"artist",
       action == Dekopon::Action::"gpt-image.edit",
       resource == Dekopon::Provider::"gpt-image")
when { context has agent && context.agent == "chat-image-studio"
    && context has via && context.via == "dekopond-gateway" };
