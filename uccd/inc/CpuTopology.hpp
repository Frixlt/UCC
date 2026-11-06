// SPDX-License-Identifier: GPL-3.0-or-later
#pragma once
#include "SysfsNode.hpp"
#include <nlohmann/json.hpp>
#include <map>
#include <sched.h>
#if defined(__i386__) || defined(__x86_64__)
#include <cpuid.h>
#endif

// Probe only online CPUs, without enabling any CPU just to identify it. Keep
// discovered types in memory so offlined E-cores retain their labels in the GUI.
class CpuTopology {
public:
  explicit CpuTopology(std::string root="/sys/devices/system/cpu"):m_root(std::move(root)) {read();}
  nlohmann::json read() {
    using Json=nlohmann::json;
    Json result=Json::array();
    const auto present=SysfsNode<std::vector<int32_t>>(m_root+"/present").read().value_or(std::vector<int32_t>{});
    for(const auto id:present) {
      const auto path=m_root+"/cpu"+std::to_string(id);
      const bool hotplug=SysfsNode<bool>(path+"/online").isAvailable();
      const bool online=!hotplug || SysfsNode<bool>(path+"/online").read().value_or(false);
      auto& cached=m_cache[id];
      if(cached.is_null()) cached=Json::object();
      if(online) {
        cached["physicalCore"]=SysfsNode<int32_t>(path+"/topology/core_id").read().value_or(id);
        cached["siblings"]=SysfsNode<std::string>(path+"/topology/thread_siblings_list").read().value_or(std::to_string(id));
        if(!cached.contains("type") || cached["type"]=="unknown") cached["type"]=probe(id);
      }
      Json row={{"id",id},{"online",online},{"hotpluggable",hotplug},{"type",cached.value("type",std::string("unknown"))},
                {"physicalCore",cached.value("physicalCore",id)},{"siblings",cached.value("siblings",std::to_string(id))}};
      result.push_back(std::move(row));
    }
    return result;
  }
private:
  std::string probe(int32_t id) const {
#if defined(__i386__) || defined(__x86_64__)
    if(m_root!="/sys/devices/system/cpu" || id<0 || id>=CPU_SETSIZE) return "unknown";
    cpu_set_t original,selected;
    if(sched_getaffinity(0,sizeof(original),&original)!=0 || !CPU_ISSET(id,&original)) return "unknown";
    CPU_ZERO(&selected);CPU_SET(id,&selected);
    if(sched_setaffinity(0,sizeof(selected),&selected)!=0) return "unknown";
    unsigned int a=0,b=0,c=0,d=0;
    if(__get_cpuid_max(0,nullptr)>=0x1a) __cpuid_count(0x1a,0,a,b,c,d);
    const int restored=sched_setaffinity(0,sizeof(original),&original);
    if(restored!=0) return "unknown";
    if((a>>24)==0x40) return "P";
    if((a>>24)==0x20) return "E";
#endif
    return "unknown";
  }
  std::string m_root;
  std::map<int32_t,nlohmann::json> m_cache;
};
