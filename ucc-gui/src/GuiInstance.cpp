#include "GuiInstance.hpp"
#include "WindowActivation.hpp"
#include <QDBusConnection>
#include <QDBusMessage>
#include <QDBusError>
#include <QDebug>
#include <QWindow>

namespace ucc {
namespace {
#ifdef UCC_READ_ONLY_PREVIEW
constexpr auto service = "com.uniwill.UccGuiPreview";
constexpr auto path = "/com/uniwill/UccGuiPreview";
#else
constexpr auto service = "com.uniwill.UccGui";
constexpr auto path = "/com/uniwill/UccGui";
#endif
constexpr auto interfaceName = "com.uniwill.UccGui";
}

GuiInstance::~GuiInstance() {
  if (m_primary) {
    auto bus = QDBusConnection::sessionBus();
    bus.unregisterService(service);
    bus.unregisterObject(path);
  }
}

GuiInstance::Result GuiInstance::startOrActivate() {
  if (m_primary) return Result::Primary;
  const auto token=takeActivationToken();
  auto bus = QDBusConnection::sessionBus();
  if (!bus.isConnected() ||
      !bus.registerObject(path, this, QDBusConnection::ExportAllSlots)) {
    qWarning() << "Cannot register the GUI session endpoint:" << bus.lastError().message();
    return Result::Error;
  }
  // D-Bus ownership is atomic, including simultaneous launches. Claim it before
  // constructing MainWindow or any hardware client.
  if (bus.registerService(service)) {
    m_primary = true;
    m_activationToken=token;m_activationPending=!token.isEmpty();
    return Result::Primary;
  }
  bus.unregisterObject(path);
  auto request = QDBusMessage::createMethodCall(service, path, interfaceName, token.isEmpty() ? "Activate" : "ActivateWithToken");
  if(!token.isEmpty()) request.setArguments({token});
  auto reply = bus.call(request, QDBus::Block, 3000);
  if(reply.type()==QDBusMessage::ErrorMessage && reply.errorName()=="org.freedesktop.DBus.Error.UnknownMethod" && !token.isEmpty()) {
    request=QDBusMessage::createMethodCall(service,path,interfaceName,"Activate");
    reply=bus.call(request,QDBus::Block,3000);
  }
  if (reply.type() == QDBusMessage::ErrorMessage) {
    qWarning() << "Could not activate the existing GUI:" << reply.errorMessage();
    return Result::Error;
  }
  return Result::ActivatedExisting;
}

void GuiInstance::setWindow(QWidget *window) {
  m_window = window;
  if (m_activationPending) ActivateWithToken(m_activationToken);
}

void GuiInstance::Activate() { ActivateWithToken({}); }
void GuiInstance::ActivateWithToken(const QString &token) {
  m_activationPending = !m_window;
  m_activationToken=token;
  if (!m_window) return;
  activateGuiWindow(m_window,token);
  m_activationToken.clear();
}
}
