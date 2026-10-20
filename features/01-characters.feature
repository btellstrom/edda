@pending
Feature: Keep character sheets

  Background:
    Given a 2024 campaign "Vox Machina" with a DM device "DM laptop"

  @justifies:SPEC101 @justifies:SPEC115 @justifies:SPEC116 @justifies:SPEC122
  Scenario: CH1 A character needs only a name
    When the DM runs "edda new Grog"
    Then the exit code is 0
    And "edda list" shows "Grog"
    And Grog's sheet has proficiency bonus 2 and no classes

  @justifies:SPEC111
  Scenario: CH2 A misspelt field is refused, not dropped
    Given the file "grog.json" contains:
      """
      { "name": "Grog", "max_hitpoints": 50 }
      """
    When the DM runs "edda import grog.json"
    Then the exit code is not 0
    And the output names the field "max_hitpoints"
    And "edda list" does not show "Grog"

  @justifies:SPEC121
  Scenario Outline: CH3 Ability modifiers round down
    Given a character "Grog" with Strength <score>
    Then Grog's Strength modifier is <modifier>

    Examples:
      | score | modifier |
      | 1     | -5       |
      | 9     | -1       |
      | 10    | 0        |
      | 15    | +2       |
      | 20    | +5       |

  @justifies:SPEC166 @justifies:SPEC167
  Scenario: CH4 Relative hit point changes
    Given a character "Grog" with 50 of 50 hit points
    When the DM runs "edda grog set hp -7"
    Then Grog has 43 of 50 hit points
    When the DM runs "edda grog set hp +10"
    Then Grog has 50 of 50 hit points

  @justifies:SPEC169
  Scenario: CH5 A character at full health stays at full health when their maximum rises
    Given a character "Grog" with 50 of 50 hit points
    When the DM runs "edda grog set max-hp 60"
    Then Grog has 60 of 60 hit points

  @justifies:SPEC148 @justifies:SPEC150
  Scenario: CH6 A short rest restores only what recharges on a short rest
    Given a character "Grog" with the feature "Rage" 3 uses recharging on a long rest, all used
    And Grog has the feature "Second Wind" 1 use recharging on a short rest, used
    When the DM runs "edda grog rest short"
    Then Grog has 0 of 3 "Rage" uses left
    And Grog has 1 of 1 "Second Wind" uses left

  @justifies:SPEC151 @justifies:SPEC153 @justifies:SPEC154
  Scenario: CH7 A long rest restores everything but what recharges by hand
    Given a character "Grog" with 20 of 50 hit points
    And Grog has the feature "Rage" 3 uses recharging on a long rest, all used
    And Grog has the feature "Boon" 1 use recharging by hand, used
    When the DM runs "edda grog rest long"
    Then Grog has 50 of 50 hit points
    And Grog has 3 of 3 "Rage" uses left
    And Grog has 0 of 1 "Boon" uses left
