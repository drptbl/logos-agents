#include "logos_agents_app_backend.h"

#include <QJsonDocument>
#include <QJsonObject>

#include "logos_sdk.h"

namespace {
constexpr int kPollMs = 2000;
constexpr int kWalletPollMs = 10000;
constexpr int kFirstPollMs = 250;
constexpr int kKeep = 200;

// logos_agents answers every method with a JSON object. Anything else (an empty
// reply when the call itself failed) becomes an error, never a silent success.
QString checked(const QString& reply)
{
    if (QJsonDocument::fromJson(reply.toUtf8()).isObject())
        return reply;
    return QStringLiteral(R"({"error":"logos_agents did not answer"})");
}

bool isError(const QString& reply)
{
    return QJsonDocument::fromJson(reply.toUtf8()).object().contains(QStringLiteral("error"));
}
}

QString LogosAgentsAppBackend::token()
{
    return checked(modules().logos_agents.token());
}

QString LogosAgentsAppBackend::rotateToken()
{
    return checked(modules().logos_agents.rotate_token());
}

QString LogosAgentsAppBackend::setPermission(QString group, bool enabled)
{
    const QString reply = checked(modules().logos_agents.set_permission(group, enabled));
    if (!isError(reply))
        setPermissions(reply);
    return reply;
}

QString LogosAgentsAppBackend::setAgentAccount(QString accountId, bool agent)
{
    const QString reply = checked(modules().logos_agents.set_agent_account(accountId, agent));
    // After replying: reading every balance first could make a saved change time out.
    QTimer::singleShot(0, [this]() { refreshWallet(); });
    return reply;
}

QString LogosAgentsAppBackend::newAgentAccount()
{
    const QString reply = checked(modules().logos_agents.new_agent_account());
    QTimer::singleShot(0, [this]() { refreshWallet(); });
    return reply;
}

QString LogosAgentsAppBackend::approve(QString requestId)
{
    const QString reply = checked(modules().logos_agents.approve(requestId));
    setApprovals(checked(modules().logos_agents.approvals()));
    return reply;
}

QString LogosAgentsAppBackend::deny(QString requestId)
{
    const QString reply = checked(modules().logos_agents.deny(requestId));
    setApprovals(checked(modules().logos_agents.approvals()));
    return reply;
}

void LogosAgentsAppBackend::refreshWallet()
{
    setWallet(checked(modules().logos_agents.wallet()));
}

QString LogosAgentsAppBackend::saveOffers(QString offers)
{
    const QString reply = checked(modules().logos_agents.set_offers(offers));
    if (!isError(reply))
        setOffers(reply);
    return reply;
}

void LogosAgentsAppBackend::refreshOffers()
{
    const QString reply = checked(modules().logos_agents.offers());
    if (!isError(reply))
        setOffers(reply);
}

void LogosAgentsAppBackend::onContextReady()
{
    QObject::connect(&m_poll, &QTimer::timeout, [this]() { refresh(); });
    m_poll.start(kPollMs);
    QObject::connect(&m_walletPoll, &QTimer::timeout, [this]() {
        refreshWallet();
        refreshOffers();
    });
    m_walletPoll.start(kWalletPollMs);
    // Not from inside the hook: on Basecamp 0.4.0-rc.2 calls made this early are
    // refused because the backend's token isn't registered yet.
    QTimer::singleShot(kFirstPollMs, [this]() {
        refresh();
        refreshWallet();
        refreshOffers();
    });
}

void LogosAgentsAppBackend::refresh()
{
    setStatus(checked(modules().logos_agents.status()));
    const QString permissions = checked(modules().logos_agents.permissions());
    if (!isError(permissions))
        setPermissions(permissions);
    // Every poll: a waiting agent gives up after a minute.
    setApprovals(checked(modules().logos_agents.approvals()));

    const QString reply = checked(modules().logos_agents.activity(m_after));
    if (isError(reply))
        return;
    const QJsonObject page = QJsonDocument::fromJson(reply.toUtf8()).object();
    const qlonglong next = page.value(QStringLiteral("next")).toInteger(m_after);
    const QJsonArray entries = page.value(QStringLiteral("entries")).toArray();
    // The log started over (its file was reset): drop what came before.
    const bool reset = next < m_after;
    if (reset)
        m_activity = QJsonArray();
    m_after = next;
    if (entries.isEmpty() && !reset)
        return;
    for (const auto& entry : entries)
        m_activity.append(entry);
    while (m_activity.size() > kKeep)
        m_activity.removeFirst();
    setActivity(QString::fromUtf8(QJsonDocument(m_activity).toJson(QJsonDocument::Compact)));
}
