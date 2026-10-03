import { mount } from "svelte";
import "../lib/base.css";
import Onboarding from "./Onboarding.svelte";

mount(Onboarding, { target: document.getElementById("app")! });
