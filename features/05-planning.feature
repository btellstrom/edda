@pending
Feature: Plan encounters

  Background:
    Given the SRD is loaded

  @justifies:SPEC402 @justifies:SPEC405
  Scenario: PL1 Under 2014 rules, numbers multiply the XP
    Given a 2014 campaign with four level-1 characters
    When the DM opens "edda plan Ambush" and enters "add 4 goblin"
    Then the plan is worth 200 XP, counted as 400
    And the plan is "Deadly" for the party

  @justifies:SPEC403
  Scenario: PL2 Under 2024 rules, plain XP meets the party's budget
    Given a 2024 campaign with four level-1 characters
    When the DM opens "edda plan Ambush" and enters "add 4 goblin warrior"
    Then the plan is worth 200 XP against budgets of Low 200, Moderate 300 and High 400
    And the plan is "Low" for the party

  @justifies:SPEC405
  Scenario: PL3 A character sitting out doesn't count
    Given a 2024 campaign with characters Grog at level 5 and Vex at level 5
    When the DM opens "edda plan Ambush" and enters "party grog"
    Then the difficulty is worked out for Vex alone

  @justifies:SPEC404
  Scenario: PL4 Adding a monster already planned adds to its entry
    Given a 2024 campaign with a DM device
    When the DM opens "edda plan Ambush" and enters "add 2 goblin warrior" and "add goblin warrior"
    Then the plan has one entry of 3 "Goblin Warrior"

  @justifies:SPEC407 @justifies:SPEC601
  Scenario: PL5 Plans are the DM's
    Given a 2024 campaign with two DM devices and a player device
    And the first DM device made the plan "Ambush"
    Then the second DM device sees the plan "Ambush"
    And the player's device is refused the campaign's plans

  @justifies:SPEC401
  Scenario: PL6 A monster without XP is worth its challenge rating's
    Given a 2024 campaign with a monster "Cave Troll" of challenge 5 without XP
    When the DM adds it to a plan
    Then it counts as 1800 XP
