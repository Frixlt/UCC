/*
 * This program is free software: you can redistribute it and/or modify
 * it under the terms of the GNU General Public License as published by
 * the Free Software Foundation, either version 3 of the License, or
 * (at your option) any later version.
 *
 * This program is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 * GNU General Public License for more details.
 *
 * You should have received a copy of the GNU General Public License
 * along with this program.  If not, see <https://www.gnu.org/licenses/>.
 */

#pragma once

#include "SysfsNode.hpp"
#include <string>
#include <vector>
#include <optional>
#include <algorithm>
#include <ranges>
#include <cmath>
#include <set>
#include <functional>

enum class ScalingDriver
{
  acpi_cpufreq,
  intel_pstate,
  amd_pstate,
  amd_pstate_epp,
  unknown
};

/**
 * @brief Controller for a single logical CPU core
 *
 * Manages sysfs interfaces for cpufreq parameters of a single core.
 */
class LogicalCpuController
{
public:
  const std::string basePath;
  const int32_t coreIndex;
  const std::string cpuPath;
  const std::string cpufreqPath;

  // cpuX/online
  SysfsNode< bool > online;

  // cpuX/cpufreq/*
  SysfsNode< int32_t > scalingCurFreq;
  SysfsNode< int32_t > scalingMinFreq;
  SysfsNode< int32_t > scalingMaxFreq;
  SysfsNode< std::vector< int32_t > > scalingAvailableFrequencies;
  SysfsNode< std::string > scalingDriver;
  SysfsNode< std::vector< std::string > > energyPerformanceAvailablePreferences;
  SysfsNode< std::string > energyPerformancePreference;
  SysfsNode< std::vector< std::string > > scalingAvailableGovernors;
  SysfsNode< std::string > scalingGovernor;
  SysfsNode< int32_t > cpuinfoMinFreq;
  SysfsNode< int32_t > cpuinfoMaxFreq;
  SysfsNode<int32_t> baseFrequency;

  explicit LogicalCpuController( const std::string &base, int32_t index )
    : basePath( base )
    , coreIndex( index )
    , cpuPath( base + "/cpu" + std::to_string( index ) )
    , cpufreqPath( cpuPath + "/cpufreq" )
    , online( cpuPath + "/online" )
    , scalingCurFreq( cpufreqPath + "/scaling_cur_freq" )
    , scalingMinFreq( cpufreqPath + "/scaling_min_freq" )
    , scalingMaxFreq( cpufreqPath + "/scaling_max_freq" )
    , scalingAvailableFrequencies( cpufreqPath + "/scaling_available_frequencies", " " )
    , scalingDriver( cpufreqPath + "/scaling_driver" )
    , energyPerformanceAvailablePreferences( cpufreqPath + "/energy_performance_available_preferences", " " )
    , energyPerformancePreference( cpufreqPath + "/energy_performance_preference" )
    , scalingAvailableGovernors( cpufreqPath + "/scaling_available_governors", " " )
    , scalingGovernor( cpufreqPath + "/scaling_governor" )
    , cpuinfoMinFreq( cpufreqPath + "/cpuinfo_min_freq" )
    , cpuinfoMaxFreq( cpufreqPath + "/cpuinfo_max_freq" )
    , baseFrequency(cpufreqPath + "/base_frequency")
  {}

  /**
   * @brief Get reduced available frequency (middle of frequency range)
   */
  std::optional< int32_t > getReducedAvailableFreq() const
  {
    auto availableFreqs = scalingAvailableFrequencies.read();

    if ( not availableFreqs.has_value() or availableFreqs->empty() )
    {
      // fallback: use average of min and max
      auto minFreq = cpuinfoMinFreq.read();
      auto maxFreq = cpuinfoMaxFreq.read();

      if ( minFreq.has_value() and maxFreq.has_value() )
      {
        return ( *minFreq + *maxFreq ) / 2;
      }

      return std::nullopt;
    }

    return ( *availableFreqs )[ availableFreqs->size() / 2 ];
  }
};

/**
 * @brief Controller for CPU frequency and governor settings
 *
 * Manages scaling governors, frequencies, energy performance preferences,
 * online cores and turbo/boost settings for all logical cores.
 */
class CpuController
{
public:
  static constexpr const char *basePath = "/sys/devices/system/cpu";
  const std::string rootPath;

  std::vector< LogicalCpuController > cores;

  // /sys/devices/system/cpu/...
  SysfsNode< int32_t > kernelMax;
  SysfsNode< std::vector< int32_t > > offline;
  SysfsNode< std::vector< int32_t > > online;
  SysfsNode< std::vector< int32_t > > possible;
  SysfsNode< std::vector< int32_t > > present;

  // intel_pstate
  SysfsNode< bool > intelPstateNoTurbo;
  SysfsNode< bool > intelHwpDynamicBoost;

  // boost
  SysfsNode< bool > boost;

  explicit CpuController(std::string root=basePath)
    : rootPath(std::move(root))
    , kernelMax( rootPath + "/kernel_max" )
    , offline( rootPath + "/offline" )
    , online( rootPath + "/online" )
    , possible( rootPath + "/possible" )
    , present( rootPath + "/present" )
    , intelPstateNoTurbo( rootPath + "/intel_pstate/no_turbo" )
    , intelHwpDynamicBoost( rootPath + "/intel_pstate/hwp_dynamic_boost" )
    , boost( rootPath + "/cpufreq/boost" )
  {
    getAvailableLogicalCores();
  }

  /**
   * @brief Discover and populate available logical CPU cores
   */
  void getAvailableLogicalCores()
  {
    cores.clear();

    auto possibleCores = possible.read();
    auto presentCores = present.read();

    if ( not possibleCores.has_value() or not presentCores.has_value() )
      return;

    std::vector< int32_t > coreIndexToAdd;

    for ( int32_t possibleIndex : *possibleCores )
    {
      if ( std::ranges::find( *presentCores, possibleIndex ) != presentCores->end() )
      {
        coreIndexToAdd.push_back( possibleIndex );
      }
    }

    std::ranges::sort( coreIndexToAdd );

    for ( int32_t coreIndex : coreIndexToAdd )
    {
      LogicalCpuController newCore( rootPath, coreIndex );

      // core 0 doesn't have online control, always include it
      cores.push_back( std::move( newCore ) );
    }
  }

  /**
   * @brief Set number of online CPU cores
   *
   * @param numberOfCores Number of cores to enable (defaults to all available)
   */
  void useCores( std::optional< int32_t > numberOfCores = std::nullopt )
  {
    if(cores.empty()) return;
    if ( not numberOfCores.has_value() )
      numberOfCores = static_cast< int32_t >( cores.size() );

    // Clamp to valid range [1, cores.size()]
    numberOfCores = std::clamp( *numberOfCores, 1, static_cast< int32_t >( cores.size() ) );

    std::vector<int32_t> selection;
    for(size_t i=0;i<cores.size();++i)
      if(static_cast<int32_t>(i)<*numberOfCores || !cores[i].online.isAvailable()) selection.push_back(cores[i].coreIndex);
    std::string error;setEnabledCores(selection,error);
  }

  bool validateEnabledCores(const std::vector<int32_t>& selection,std::string& error) const {
    const std::set<int32_t> ids(selection.begin(),selection.end());
    if(ids.empty() || ids.size()!=selection.size()) {error="CPU selection is empty or contains duplicates";return false;}
    for(const auto id:ids)
      if(std::ranges::none_of(cores,[id](const auto& c){return c.coreIndex==id;})) {error="Unknown CPU "+std::to_string(id);return false;}
    for(const auto& core:cores)
      if(!core.online.isAvailable() && !ids.contains(core.coreIndex)) {error="CPU "+std::to_string(core.coreIndex)+" cannot be disabled";return false;}
    return true;
  }
  using OnlineWriter=std::function<bool(LogicalCpuController&,bool)>;
  bool setEnabledCores(const std::vector<int32_t>& selection,std::string& error,OnlineWriter writer={}) {
    if(!validateEnabledCores(selection,error)) return false;
    const std::set<int32_t> ids(selection.begin(),selection.end());
    if(!writer) writer=[](auto& core,bool value){return core.online.write(value);};
    // Bring all requested CPUs online before disabling anything else.
    for(bool enable:{true,false}) for(auto& core:cores) {
      if(!core.online.isAvailable() || ids.contains(core.coreIndex)!=enable) continue;
      if(core.online.read().value_or(!enable)==enable) continue;
      if(!writer(core,enable)) {error="Cannot change CPU "+std::to_string(core.coreIndex);return false;}
    }
    return true;
  }

  /**
   * @brief Find closest value in a sorted vector
   */
  static int32_t findClosestValue( int32_t target, const std::vector< int32_t > &values )
  {
    if ( values.empty() )
      return target;

    auto it = std::lower_bound( values.begin(), values.end(), target );

    if ( it == values.end() )
      return values.back();

    if ( it == values.begin() )
      return values.front();

    // check if previous value is closer
    auto prev = std::prev( it );

    if ( std::abs( *it - target ) < std::abs( *prev - target ) )
      return *it;

    return *prev;
  }

  static std::optional<int32_t> effectiveHardwareMaxFreq(const LogicalCpuController& core) {
    auto maximum=core.cpuinfoMaxFreq.read();
    if(maximum && SysfsNode<bool>(core.basePath+"/intel_pstate/no_turbo").read().value_or(false))
      if(auto base=core.baseFrequency.read();base && *base>0) maximum=std::min(*maximum,*base);
    return maximum;
  }

  /**
   * @brief Compute the effective max scaling frequency that will be written to a core.
   *
   * Mirrors the per-core clamping and frequency-snapping logic of
   * setGovernorScalingMaxFrequency() so that validation and writing always agree
   * on the expected value regardless of per-core hardware limits (e.g. the best
   * P-core vs other P-cores vs E-cores all have different cpuinfo_max_freq on
   * heterogeneous Intel/AMD CPUs with Turbo Boost Max 3.0 or hybrid topologies).
   *
   * @param core The logical core to compute for
   * @param targetMax The requested maximum frequency (same semantics as setGovernorScalingMaxFrequency)
   * @param acpiFallback True when scaling driver is acpi-cpufreq and boost is available
   * @return The effective frequency that will be set or nullopt if sysfs nodes are unavailable
   */
  static std::optional< int32_t > computeEffectiveMaxFreq( const LogicalCpuController &core,
                                                           std::optional< int32_t > targetMax,
                                                           bool acpiFallback )
  {
    if ( not core.scalingMinFreq.isAvailable() or not core.scalingMaxFreq.isAvailable()
         or not core.cpuinfoMinFreq.isAvailable() or not core.cpuinfoMaxFreq.isAvailable() )
      return std::nullopt;

    auto coreMin = core.cpuinfoMinFreq.read();
    auto coreMax = effectiveHardwareMaxFreq(core);
    auto scalingMin = core.scalingMinFreq.read();
    auto avail = core.scalingAvailableFrequencies.read();

    if ( not coreMin or not coreMax or not scalingMin )
      return std::nullopt;

    scalingMin=std::min(*scalingMin,*coreMax);

    int32_t freq;

    if ( not targetMax.has_value() )
    {
      freq = *coreMax;
    }
    else if ( *targetMax == -1 )
    {
      // special case: -1 means "reduced"
      if ( acpiFallback )
        freq = *coreMax;
      else
      {
        auto reduced = core.getReducedAvailableFreq();
        freq = reduced.value_or( *coreMax );
      }
    }
    else
    {
      freq = *targetMax;
    }

    // clamp to per-core hardware limits
    freq = std::clamp( freq, *scalingMin, *coreMax );

    // snap to the closest available frequency (filtered to >= scalingMin)
    if ( avail and not avail->empty() )
    {
      std::vector< int32_t > filtered;
      for ( int32_t f : *avail )
        if ( f >= *scalingMin )
          filtered.push_back( f );
      if ( not filtered.empty() )
        freq = findClosestValue( freq, filtered );
    }

    return freq;
  }

  /**
   * @brief Compute the effective min scaling frequency that will be written to a core.
   *
   * Mirrors the per-core clamping and frequency-snapping logic of
   * setGovernorScalingMinFrequency().
   *
   * @param core The logical core to compute for
   * @param targetMin The requested minimum frequency (same semantics as setGovernorScalingMinFrequency)
   * @return The effective frequency that will be set or nullopt if sysfs nodes are unavailable
   */
  static std::optional< int32_t > computeEffectiveMinFreq( const LogicalCpuController &core,
                                                           std::optional< int32_t > targetMin )
  {
    if ( not core.scalingMinFreq.isAvailable() or not core.scalingMaxFreq.isAvailable()
         or not core.cpuinfoMinFreq.isAvailable() or not core.cpuinfoMaxFreq.isAvailable() )
      return std::nullopt;

    auto coreMin = core.cpuinfoMinFreq.read();
    auto coreMax = effectiveHardwareMaxFreq(core);
    auto scalingMax = core.scalingMaxFreq.read();
    auto avail = core.scalingAvailableFrequencies.read();

    if ( not coreMin or not coreMax or not scalingMax )
      return std::nullopt;
    scalingMax=std::min(*scalingMax,*coreMax);
    if(*coreMin>*scalingMax) return std::nullopt;

    int32_t freq;

    if ( not targetMin.has_value() )
    {
      freq = *coreMin;
    }
    else if ( *targetMin == -2 )
    {
      // special case: -2 means "set to max"
      freq = *coreMax;
    }
    else
    {
      freq = std::clamp( *targetMin, *coreMin, *scalingMax );
    }

    // snap to the closest available frequency (filtered to <= scalingMax)
    if ( avail and not avail->empty() )
    {
      std::vector< int32_t > filtered;
      for ( int32_t f : *avail )
        if ( f <= *scalingMax )
          filtered.push_back( f );
      if ( not filtered.empty() )
        freq = findClosestValue( freq, filtered );
    }

    return freq;
  }

  /**
   * @brief Set maximum scaling frequency for all cores
   *
   * @param setMaxFrequency Target frequency (-1 for reduced, undefined for max)
   */
  void setGovernorScalingMaxFrequency( std::optional< int32_t > setMaxFrequency = std::nullopt )
  {
    std::optional< std::string > scalingDriverStr;
    bool acpiFallback = false;

    for ( auto &core : cores )
    {
      if ( not core.scalingMinFreq.isAvailable() or not core.scalingMaxFreq.isAvailable()
           or not core.cpuinfoMinFreq.isAvailable() or not core.cpuinfoMaxFreq.isAvailable() )
        continue;

      if ( core.coreIndex != 0 and not core.online.read().value_or( false ) )
        continue;

      if ( not scalingDriverStr.has_value() )
      {
        scalingDriverStr = core.scalingDriver.read();
        acpiFallback = boost.isAvailable() and scalingDriverStr.has_value()
                       and *scalingDriverStr == "acpi-cpufreq";
      }

      auto effective = computeEffectiveMaxFreq( core, setMaxFrequency, acpiFallback );
      if ( effective )
        core.scalingMaxFreq.write( *effective );
    }

    // handle boost for AMD (boost not included in max frequency)
    if ( cores.empty() )
      return;

    auto maxFrequency = cores[ 0 ].cpuinfoMaxFreq.read();
    auto availableFrequencies = cores[ 0 ].scalingAvailableFrequencies.read();

    int32_t maximumAvailableFrequency = maxFrequency.value_or( 0 );

    if ( availableFrequencies.has_value() and not availableFrequencies->empty() )
    {
      maximumAvailableFrequency = availableFrequencies->front();
    }

    if ( boost.isAvailable() and scalingDriverStr.has_value() and *scalingDriverStr == "acpi-cpufreq" )
    {
      if ( not setMaxFrequency.has_value() or *setMaxFrequency > maximumAvailableFrequency )
      {
        boost.write( true );
      }
      else
      {
        boost.write( false );
      }
    }
  }

  /**
   * @brief Set minimum scaling frequency for all cores
   *
   * @param setMinFrequency Target frequency (-2 for max, undefined for min)
   */
  void setGovernorScalingMinFrequency( std::optional< int32_t > setMinFrequency = std::nullopt )
  {
    for ( auto &core : cores )
    {
      if ( not core.scalingMinFreq.isAvailable() or not core.scalingMaxFreq.isAvailable()
           or not core.cpuinfoMinFreq.isAvailable() or not core.cpuinfoMaxFreq.isAvailable() )
        continue;

      if ( core.coreIndex != 0 and core.online.read().value_or( false ) == false )
        continue;

      auto effective = computeEffectiveMinFreq( core, setMinFrequency );
      if ( effective )
        core.scalingMinFreq.write( *effective );
    }
  }

  /**
   * @brief Set scaling governor for all cores
   */
  void setGovernor( const std::optional< std::string > &governor )
  {
    if ( not governor.has_value() )
      return;

    for ( auto &core : cores )
    {
      if ( not core.scalingGovernor.isAvailable() or not core.scalingAvailableGovernors.isAvailable() )
        continue;

      if ( core.coreIndex != 0 and core.online.read().value_or( false ) == false )
        continue;

      auto availableGovernors = core.scalingAvailableGovernors.read();

      if ( not availableGovernors.has_value() )
        continue;

      if ( std::ranges::find( *availableGovernors, *governor ) != availableGovernors->end() )
      {
        core.scalingGovernor.write( *governor );
      }
    }
  }

  /**
   * @brief Set energy performance preference for all cores
   */
  void setEnergyPerformancePreference( const std::optional< std::string > &preference )
  {
    if ( not preference.has_value() )
      return;

    for ( auto &core : cores )
    {
      if ( not core.energyPerformancePreference.isAvailable()
           or not core.energyPerformanceAvailablePreferences.isAvailable() )
        continue;

      if ( core.coreIndex != 0 and core.online.read().value_or( false ) == false )
        continue;

      auto availablePreferences = core.energyPerformanceAvailablePreferences.read();

      if ( not availablePreferences.has_value() )
        continue;

      if ( std::ranges::find( *availablePreferences, *preference ) != availablePreferences->end() )
      {
        core.energyPerformancePreference.write( *preference );
      }
    }
  }

  /**
   * @brief Get scaling driver enum from string
   */
  static ScalingDriver getScalingDriverEnum( const std::string &driver )
  {
    if ( driver == "acpi-cpufreq" )
      return ScalingDriver::acpi_cpufreq;

    if ( driver == "intel_pstate" )
      return ScalingDriver::intel_pstate;

    if ( driver == "amd-pstate" )
      return ScalingDriver::amd_pstate;

    if ( driver == "amd-pstate-epp" )
      return ScalingDriver::amd_pstate_epp;

    return ScalingDriver::unknown;
  }
};
