import { mount } from "svelte";
import "../lib/base.css";
import Settings from "./Settings.svelte";

mount(Settings, { target: document.getElementById("app")! });
